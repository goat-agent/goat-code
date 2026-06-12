use std::cell::RefCell;

use goat_protocol::{TaskId, ToolCall, ToolCallId, ToolDisplay, ToolOutcome};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{highlight::Highlighter, layout::format_tokens, markdown, symbols, theme::Theme, wrap};

pub(crate) struct Working {
    pub elapsed: Option<u64>,
    pub label: Option<String>,
    pub thinking: bool,
    pub tokens: Option<u64>,
}

pub(crate) struct RenderCtx<'a> {
    pub theme: Theme,
    pub scroll: usize,
    pub spinner: &'static str,
    pub working: Option<&'a Working>,
    pub queued: &'a [String],
    pub hl: &'a dyn Highlighter,
}

struct RenderCache {
    width: u16,
    version: u64,
    lines: Vec<Line<'static>>,
    spinner_lines: Vec<usize>,
}

struct StreamCache {
    len: usize,
    width: u16,
    rows: Vec<Line<'static>>,
}

#[derive(Debug)]
pub(crate) enum ToolStatus {
    Running,
    Done(ToolOutcome),
}

#[derive(Debug)]
pub(crate) enum ShellStatus {
    Running,
    Done(String),
}

#[derive(Debug)]
pub(crate) enum Item {
    User(String),
    Agent(Vec<Line<'static>>),
    Tool {
        id: ToolCallId,
        name: String,
        display: ToolDisplay,
        status: ToolStatus,
    },
    Shell {
        id: TaskId,
        command: String,
        status: ShellStatus,
    },
    Error(String),
    Notice(String),
    Compaction {
        tokens_before: u32,
        tokens_after: u32,
    },
}

#[derive(Default)]
pub struct Transcript {
    pub(crate) items: Vec<Item>,
    streaming: Option<String>,
    version: u64,
    cache: RefCell<Option<RenderCache>>,
    stream_cache: RefCell<Option<StreamCache>>,
}

impl Transcript {
    fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
        *self.cache.borrow_mut() = None;
        *self.stream_cache.borrow_mut() = None;
    }

    pub fn invalidate(&mut self) {
        self.bump_version();
    }

    pub fn clear(&mut self) {
        self.bump_version();
        self.items.clear();
        self.streaming = None;
    }

    pub fn push_user(&mut self, text: impl Into<String>) {
        self.bump_version();
        self.items.push(Item::User(text.into()));
    }

    pub fn push_delta(&mut self, chunk: &str) {
        self.streaming
            .get_or_insert_with(String::new)
            .push_str(chunk);
    }

    pub fn commit_text(&mut self, text: &str, hl: &dyn Highlighter, theme: Theme) {
        self.bump_version();
        self.streaming = None;
        self.items
            .push(Item::Agent(markdown::render(text, theme, hl)));
    }

    pub fn push_tool(&mut self, call: ToolCall) {
        self.bump_version();
        self.items.push(Item::Tool {
            id: call.id,
            name: call.name,
            display: call.display,
            status: ToolStatus::Running,
        });
    }

    pub fn finish_tool(&mut self, call_id: ToolCallId, outcome: ToolOutcome) {
        self.bump_version();
        for item in self.items.iter_mut().rev() {
            if let Item::Tool { id, status, .. } = item
                && *id == call_id
                && matches!(status, ToolStatus::Running)
            {
                *status = ToolStatus::Done(outcome);
                return;
            }
        }
    }

    pub fn push_shell(&mut self, id: TaskId, command: String) {
        self.bump_version();
        self.items.push(Item::Shell {
            id,
            command,
            status: ShellStatus::Running,
        });
    }

    pub fn finish_shell(&mut self, task_id: TaskId, output: String) {
        self.bump_version();
        for item in self.items.iter_mut().rev() {
            if let Item::Shell { id, status, .. } = item
                && *id == task_id
                && matches!(status, ShellStatus::Running)
            {
                *status = ShellStatus::Done(output);
                return;
            }
        }
    }

    pub fn push_error(&mut self, text: impl Into<String>, hl: &dyn Highlighter, theme: Theme) {
        self.bump_version();
        if let Some(buffer) = self.streaming.take() {
            let text = format!("{buffer} {} stopped", symbols::ui::ELLIPSIS);
            self.items
                .push(Item::Agent(markdown::render(&text, theme, hl)));
        }
        self.items.push(Item::Error(text.into()));
    }

    pub fn discard_stream(&mut self) {
        self.bump_version();
        self.streaming = None;
    }

    pub fn push_compaction(&mut self, tokens_before: u32, tokens_after: u32) {
        self.bump_version();
        self.items.push(Item::Compaction {
            tokens_before,
            tokens_after,
        });
    }

    pub fn complete(&mut self, interrupted: bool, hl: &dyn Highlighter, theme: Theme) {
        self.bump_version();
        if interrupted {
            for item in &mut self.items {
                if let Item::Tool { status, .. } = item
                    && matches!(status, ToolStatus::Running)
                {
                    *status = ToolStatus::Done(ToolOutcome {
                        ok: false,
                        summary: None,
                    });
                }
                if let Item::Shell { status, .. } = item
                    && matches!(status, ShellStatus::Running)
                {
                    *status = ShellStatus::Done(String::new());
                }
            }
        }
        if let Some(buffer) = self.streaming.take() {
            let text = if interrupted {
                format!("{buffer} … interrupted")
            } else {
                buffer
            };
            self.items
                .push(Item::Agent(markdown::render(&text, theme, hl)));
        } else if interrupted && !matches!(self.items.last(), Some(Item::Error(_))) {
            self.items.push(Item::Notice("interrupted".into()));
        }
    }

    fn ensure_cache(&self, theme: Theme, width: u16) {
        let valid = self
            .cache
            .borrow()
            .as_ref()
            .is_some_and(|c| c.width == width && c.version == self.version);
        if valid {
            return;
        }
        let (lines, spinner_lines) = build_static_lines(&self.items, theme, width);
        *self.cache.borrow_mut() = Some(RenderCache {
            width,
            version: self.version,
            lines,
            spinner_lines,
        });
    }

    fn streaming_rows(&self, theme: Theme, width: u16, hl: &dyn Highlighter) -> Vec<Line<'static>> {
        let Some(buffer) = &self.streaming else {
            return Vec::new();
        };
        {
            let guard = self.stream_cache.borrow();
            if let Some(cached) = guard.as_ref()
                && cached.len == buffer.len()
                && cached.width == width
            {
                return cached.rows.clone();
            }
        }
        let mut content = markdown::render(buffer, theme, hl);
        while content.last().is_some_and(is_blank) {
            content.pop();
        }
        if let Some(last) = content.last_mut() {
            last.spans
                .push(Span::styled(symbols::ui::STREAM_CURSOR, theme.accent()));
        }
        let rows = hang(
            &content,
            Span::styled(symbols::marker::AGENT, theme.role_agent()),
            width,
        );
        *self.stream_cache.borrow_mut() = Some(StreamCache {
            len: buffer.len(),
            width,
            rows: rows.clone(),
        });
        rows
    }

    #[allow(clippy::too_many_arguments)]
    fn tail_rows(
        &self,
        theme: Theme,
        width: u16,
        hl: &dyn Highlighter,
        spinner: &'static str,
        working: Option<&Working>,
        queued: &[String],
        base_nonempty: bool,
    ) -> Vec<Line<'static>> {
        let mut tail: Vec<Line<'static>> = Vec::new();
        let streamed = self.streaming_rows(theme, width, hl);
        if !streamed.is_empty() {
            if base_nonempty {
                tail.push(Line::default());
            }
            tail.extend(streamed);
        }
        if let Some(w) = working {
            if base_nonempty || !tail.is_empty() {
                tail.push(Line::default());
            }
            tail.extend(working_rows(theme, width, spinner, w));
        }
        if !queued.is_empty() {
            if working.is_none() && (base_nonempty || !tail.is_empty()) {
                tail.push(Line::default());
            }
            tail.extend(queued_rows(theme, width, queued));
        }
        tail
    }

    pub fn content_height(
        &self,
        width: u16,
        theme: Theme,
        hl: &dyn Highlighter,
        working: Option<&Working>,
        queued: &[String],
    ) -> usize {
        self.ensure_cache(theme, width);
        let base = self.cache.borrow().as_ref().map_or(0, |c| c.lines.len());
        base + self
            .tail_rows(
                theme,
                width,
                hl,
                symbols::SPINNER[0],
                working,
                queued,
                base > 0,
            )
            .len()
    }

    pub(crate) fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
        self.ensure_cache(ctx.theme, area.width);
        let guard = self.cache.borrow();
        let Some(cache) = guard.as_ref() else {
            return;
        };
        let tail = self.tail_rows(
            ctx.theme,
            area.width,
            ctx.hl,
            ctx.spinner,
            ctx.working,
            ctx.queued,
            !cache.lines.is_empty(),
        );
        let total = cache.lines.len() + tail.len();
        let height = usize::from(area.height);
        let start = ctx.scroll.min(total.saturating_sub(height));
        let end = (start + height).min(total);
        let mut visible: Vec<Line<'static>> = Vec::with_capacity(end.saturating_sub(start));
        let static_end = end.min(cache.lines.len());
        for i in start.min(static_end)..static_end {
            let mut line = cache.lines[i].clone();
            if cache.spinner_lines.binary_search(&i).is_ok()
                && let Some(span) = line.spans.first_mut()
            {
                *span = Span::styled(ctx.spinner, ctx.theme.accent());
            }
            visible.push(line);
        }
        if end > cache.lines.len() {
            let from = start.saturating_sub(cache.lines.len());
            let to = end - cache.lines.len();
            visible.extend(tail.into_iter().take(to).skip(from));
        }
        frame.render_widget(Paragraph::new(visible), area);
    }
}

fn is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|s| s.content.is_empty())
}

fn hang(content: &[Line<'static>], marker: Span<'static>, width: u16) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2);
    let mut first = Some(marker);
    if content.is_empty() {
        return vec![Line::from(vec![first.take().unwrap_or_default()])];
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in content {
        for mut row in wrap::wrap_line(line, inner) {
            let prefix = first.take().unwrap_or_else(|| Span::raw("  "));
            row.spans.insert(0, prefix);
            out.push(row);
        }
    }
    out
}

fn plain_lines(text: &str, theme: Theme) -> Vec<Line<'static>> {
    text.split('\n')
        .map(|raw| Line::from(Span::styled(raw.to_owned(), theme.base())))
        .collect()
}

fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

const QUEUED_ROW_CAP: usize = 3;

fn queued_rows(theme: Theme, width: u16, queued: &[String]) -> Vec<Line<'static>> {
    let inner = usize::from(width.saturating_sub(2));
    let mut rows: Vec<Line<'static>> = Vec::new();
    for label in queued.iter().take(QUEUED_ROW_CAP) {
        rows.push(Line::from(vec![
            Span::styled(symbols::marker::USER, theme.muted()),
            Span::styled(truncate_to_width(label, inner), theme.muted()),
        ]));
    }
    if queued.len() > QUEUED_ROW_CAP {
        rows.push(Line::from(Span::styled(
            format!(
                "{} {} more queued",
                symbols::ui::ELLIPSIS,
                queued.len() - QUEUED_ROW_CAP
            ),
            theme.muted(),
        )));
    }
    rows
}

fn working_rows(
    theme: Theme,
    width: u16,
    spinner: &'static str,
    w: &Working,
) -> Vec<Line<'static>> {
    let label = w.label.clone().unwrap_or_else(|| {
        let verb = if w.thinking { "thinking" } else { "working" };
        format!("{verb}{}", symbols::ui::ELLIPSIS)
    });
    let mut spans = vec![Span::styled(label, theme.muted())];
    if let Some(secs) = w.elapsed {
        spans.push(Span::styled(
            format!("{}{}", symbols::ui::SEPARATOR, format_elapsed(secs)),
            theme.muted(),
        ));
    }
    if let Some(tokens) = w.tokens {
        spans.push(Span::styled(
            format!("{}{} tok", symbols::ui::SEPARATOR, format_tokens(tokens)),
            theme.muted(),
        ));
    }
    hang(
        &[Line::from(spans)],
        Span::styled(format!("{spinner} "), theme.accent()),
        width,
    )
}

fn build_static_lines(
    items: &[Item],
    theme: Theme,
    width: u16,
) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spinner_lines: Vec<usize> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            let prev_is_tool = matches!(items.get(i - 1), Some(Item::Tool { .. }));
            let cur_is_tool = matches!(item, Item::Tool { .. });
            if !(prev_is_tool && cur_is_tool) {
                lines.push(Line::default());
            }
        }
        if matches!(
            item,
            Item::Tool {
                status: ToolStatus::Running,
                ..
            } | Item::Shell {
                status: ShellStatus::Running,
                ..
            }
        ) {
            spinner_lines.push(lines.len());
        }
        lines.extend(item_rows(item, theme, width));
    }
    (lines, spinner_lines)
}

fn item_rows(item: &Item, theme: Theme, width: u16) -> Vec<Line<'static>> {
    match item {
        Item::User(text) => hang(
            &plain_lines(text, theme),
            Span::styled(symbols::marker::USER, theme.role_user()),
            width,
        ),
        Item::Agent(rendered) => {
            let end = rendered
                .iter()
                .rposition(|l| !is_blank(l))
                .map_or(0, |i| i + 1);
            hang(
                &rendered[..end],
                Span::styled(symbols::marker::AGENT, theme.role_agent()),
                width,
            )
        }
        Item::Shell {
            command, status, ..
        } => shell_rows(command, status, theme, width),
        Item::Error(text) => hang(
            &plain_lines(text, theme),
            Span::styled(symbols::marker::ERROR, theme.error()),
            width,
        ),
        Item::Notice(text) => hang(
            &plain_lines(text, theme),
            Span::styled(symbols::marker::NOTICE, theme.muted()),
            width,
        ),
        Item::Compaction {
            tokens_before,
            tokens_after,
        } => vec![Line::from(Span::styled(
            format!(
                "{} context compacted{}{} → {} {}",
                symbols::ui::RULE,
                symbols::ui::SEPARATOR,
                format_tokens(u64::from(*tokens_before)),
                format_tokens(u64::from(*tokens_after)),
                symbols::ui::RULE,
            ),
            theme.muted(),
        ))],
        Item::Tool {
            name,
            display,
            status,
            ..
        } => {
            let (marker, marker_style): (&str, _) = match status {
                ToolStatus::Running => (symbols::SPINNER[0], theme.accent()),
                ToolStatus::Done(ToolOutcome { ok: true, .. }) => {
                    (symbols::ui::CHECK, theme.role_tool())
                }
                ToolStatus::Done(ToolOutcome { ok: false, .. }) => {
                    (symbols::ui::CROSS, theme.error())
                }
            };

            let name_w = name.width();
            let avail = usize::from(width)
                .saturating_sub(2)
                .saturating_sub(name_w)
                .saturating_sub(2);

            let primary = truncate_to_width(&display.primary, avail);
            let detail_avail = avail
                .saturating_sub(primary.width())
                .saturating_sub(symbols::ui::SEPARATOR.width());
            let detail = display
                .detail
                .as_deref()
                .filter(|_| detail_avail > 1)
                .map(|d| truncate_to_width(d, detail_avail));

            let mut spans = vec![
                Span::styled(marker, marker_style),
                Span::raw(" "),
                Span::styled(name.clone(), theme.role_tool()),
                Span::styled("(", theme.muted()),
                Span::styled(primary, theme.base()),
            ];
            if let Some(d) = detail {
                spans.push(Span::styled(symbols::ui::SEPARATOR, theme.muted()));
                spans.push(Span::styled(d, theme.muted()));
            }
            spans.push(Span::styled(")", theme.muted()));

            let mut result = vec![Line::from(spans)];
            if let ToolStatus::Done(ToolOutcome {
                summary: Some(summary),
                ..
            }) = status
            {
                result.extend(result_rows(summary, theme, width));
            }
            result
        }
    }
}

const RESULT_BLOCK_CAP: usize = 6;
const SHELL_BLOCK_CAP: usize = 20;
const SHELL_EXIT_PREFIX: &str = "exit code: ";
const SHELL_NO_OUTPUT: &str = "(no output)";

fn resolve_carriage_returns(line: &str) -> &str {
    let line = line.strip_suffix('\r').unwrap_or(line);
    line.rsplit('\r').next().unwrap_or(line)
}

fn strip_control_sequences(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn sanitize_shell_output(output: &str) -> Vec<String> {
    let mut lines: Vec<String> = output
        .split('\n')
        .map(|line| strip_control_sequences(resolve_carriage_returns(line)))
        .collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines
}

fn shell_line_style(line: &str, theme: Theme) -> Style {
    if line.starts_with(SHELL_EXIT_PREFIX) || (line.starts_with('[') && line.ends_with(']')) {
        theme.error()
    } else if line.starts_with("+ ") {
        theme.role_agent()
    } else if line.starts_with("- ") {
        theme.error()
    } else {
        theme.muted()
    }
}

fn shell_rows(command: &str, status: &ShellStatus, theme: Theme, width: u16) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2);
    let (marker, marker_style) = match status {
        ShellStatus::Running => (symbols::SPINNER[0], theme.accent()),
        ShellStatus::Done(_) => (symbols::ui::BANG, theme.shell()),
    };
    let mut out = hang(
        &plain_lines(command, theme),
        Span::styled(format!("{marker} "), marker_style),
        width,
    );

    let ShellStatus::Done(output) = status else {
        return out;
    };
    let lines = sanitize_shell_output(output);
    if lines.is_empty() {
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(SHELL_NO_OUTPUT, theme.muted()),
        ]));
        return out;
    }
    let exit_line = lines
        .last()
        .filter(|line| line.starts_with(SHELL_EXIT_PREFIX))
        .cloned();
    let mut rows: Vec<Line<'static>> = Vec::new();
    for line in &lines {
        let content = Line::from(Span::styled(
            line.replace('\t', "  "),
            shell_line_style(line, theme),
        ));
        for mut row in wrap::wrap_line(&content, inner) {
            row.spans.insert(0, Span::raw("  "));
            rows.push(row);
        }
    }
    let total = rows.len();
    if total > SHELL_BLOCK_CAP {
        rows.truncate(SHELL_BLOCK_CAP);
        rows.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} {} more", symbols::ui::ELLIPSIS, total - SHELL_BLOCK_CAP),
                theme.muted(),
            ),
        ]));
        if let Some(exit) = exit_line {
            rows.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(exit, theme.error()),
            ]));
        }
    }
    out.extend(rows);
    out
}

fn result_rows(summary: &str, theme: Theme, width: u16) -> Vec<Line<'static>> {
    let src: Vec<&str> = summary.lines().collect();
    let inner = width.saturating_sub(2);
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in src.iter().take(RESULT_BLOCK_CAP) {
        let style = if line.starts_with("+ ") {
            theme.role_agent()
        } else if line.starts_with("- ") {
            theme.error()
        } else {
            theme.muted()
        };
        let content = Line::from(Span::styled(line.replace('\t', "  "), style));
        for mut row in wrap::wrap_line(&content, inner) {
            row.spans.insert(0, Span::raw("  "));
            out.push(row);
        }
    }
    if src.len() > RESULT_BLOCK_CAP {
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "{} {} more",
                    symbols::ui::ELLIPSIS,
                    src.len() - RESULT_BLOCK_CAP
                ),
                theme.muted(),
            ),
        ]));
    }
    out
}

fn truncate_to_width(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if s.width() <= max_width {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > max_width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use goat_protocol::{TaskId, ToolCall, ToolCallId, ToolOutcome};
    use ratatui::{Terminal, backend::TestBackend};

    use super::{
        Item, SHELL_BLOCK_CAP, ShellStatus, ToolStatus, Transcript, Working, format_elapsed,
        sanitize_shell_output, shell_rows,
    };
    use crate::{highlight::PlainHighlighter, symbols, theme::Theme};

    fn call(id: u64, name: &str, input: &str) -> ToolCall {
        ToolCall {
            id: ToolCallId(id),
            name: name.to_owned(),
            display: goat_protocol::ToolDisplay::primary(input),
        }
    }

    fn ok() -> ToolOutcome {
        ToolOutcome {
            ok: true,
            summary: None,
        }
    }

    fn failed(summary: &str) -> ToolOutcome {
        ToolOutcome {
            ok: false,
            summary: Some(summary.to_owned()),
        }
    }

    fn commit(t: &mut Transcript, text: &str) {
        t.commit_text(text, &PlainHighlighter, Theme::dark());
    }

    fn height(t: &Transcript, width: u16) -> usize {
        t.content_height(width, Theme::dark(), &PlainHighlighter, None, &[])
    }

    fn buffer_row(terminal: &Terminal<TestBackend>, y: u16) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn tool_lifecycle() {
        let mut t = Transcript::default();
        t.push_tool(call(1, "Read", "src/lib.rs"));
        assert!(matches!(
            &t.items[0],
            Item::Tool {
                status: ToolStatus::Running,
                ..
            }
        ));
        t.finish_tool(ToolCallId(1), ok());
        assert!(matches!(&t.items[0], Item::Tool { status: ToolStatus::Done(o), .. } if o.ok));
    }

    #[test]
    fn tool_failed_with_summary() {
        let mut t = Transcript::default();
        t.push_tool(call(2, "Bash", "cargo build"));
        t.finish_tool(ToolCallId(2), failed("error[E0308]"));
        if let Some(Item::Tool {
            status: ToolStatus::Done(outcome),
            ..
        }) = t.items.last()
        {
            assert!(!outcome.ok);
            assert_eq!(outcome.summary.as_deref(), Some("error[E0308]"));
        } else {
            panic!("expected done tool");
        }
    }

    #[test]
    fn agent_loop_ordering() {
        let mut t = Transcript::default();
        t.push_user("hi");
        t.push_delta("step one");
        commit(&mut t, "step one");
        t.push_tool(call(1, "Read", "src/lib.rs"));
        t.finish_tool(ToolCallId(1), ok());
        t.push_delta("step two");
        commit(&mut t, "step two");

        assert!(matches!(&t.items[0], Item::User(_)));
        assert!(matches!(&t.items[1], Item::Agent(_)));
        assert!(matches!(&t.items[2], Item::Tool { .. }));
        assert!(matches!(&t.items[3], Item::Agent(_)));
    }

    #[test]
    fn complete_interrupted_clears_running_tools() {
        let mut t = Transcript::default();
        t.push_tool(call(5, "Bash", "long cmd"));
        t.complete(true, &PlainHighlighter, Theme::dark());
        if let Some(Item::Tool {
            status: ToolStatus::Done(o),
            ..
        }) = t.items.first()
        {
            assert!(!o.ok);
        } else {
            panic!("expected failed tool");
        }
        assert!(
            matches!(t.items.last(), Some(Item::Notice(_))),
            "interrupt with no stream must append Notice"
        );
    }

    #[test]
    fn finish_tool_by_id_reverse_order() {
        let mut t = Transcript::default();
        t.push_tool(call(10, "Read", "a"));
        t.push_tool(call(11, "Grep", "b"));
        t.finish_tool(ToolCallId(11), ok());
        t.finish_tool(ToolCallId(10), failed("err"));
        assert!(matches!(&t.items[0], Item::Tool { status: ToolStatus::Done(o), .. } if !o.ok));
        assert!(matches!(&t.items[1], Item::Tool { status: ToolStatus::Done(o), .. } if o.ok));
    }

    #[test]
    fn content_height_counts_streaming() {
        let mut t = Transcript::default();
        commit(&mut t, "hello world");
        let h1 = height(&t, 80);
        t.push_delta("line one\nline two\nline three\nline four");
        let h2 = height(&t, 80);
        assert!(
            h2 > h1,
            "content_height must grow while streaming is active"
        );
    }

    #[test]
    fn content_height_includes_working_line() {
        let mut t = Transcript::default();
        commit(&mut t, "hello world");
        let idle = height(&t, 80);
        let working = Working {
            elapsed: None,
            label: None,
            thinking: false,
            tokens: None,
        };
        let busy = t.content_height(80, Theme::dark(), &PlainHighlighter, Some(&working), &[]);
        assert!(
            busy > idle,
            "content_height must be larger when busy (working line)"
        );
    }

    #[test]
    fn interrupted_without_stream_pushes_notice() {
        let mut t = Transcript::default();
        t.complete(true, &PlainHighlighter, Theme::dark());
        assert!(
            matches!(t.items.last(), Some(Item::Notice(_))),
            "interrupting with no stream must push a Notice item"
        );
    }

    #[test]
    fn error_commits_partial_stream_before_error_row() {
        let mut t = Transcript::default();
        t.push_delta("partial answer");
        t.push_error("boom", &PlainHighlighter, Theme::dark());
        assert!(matches!(&t.items[0], Item::Agent(_)));
        assert!(matches!(&t.items[1], Item::Error(_)));
        t.complete(true, &PlainHighlighter, Theme::dark());
        assert!(
            !matches!(t.items.last(), Some(Item::Notice(_))),
            "interrupted notice must be suppressed right after an error row"
        );
    }

    #[test]
    fn discard_stream_drops_partial_only() {
        let mut t = Transcript::default();
        commit(&mut t, "committed");
        t.push_delta("doomed partial");
        let before = height(&t, 80);
        t.discard_stream();
        assert!(height(&t, 80) < before);
        assert_eq!(t.items.len(), 1);
    }

    #[test]
    fn word_wrapped_bottom_is_visible_at_max_scroll() {
        let mut t = Transcript::default();
        commit(&mut t, "aaaaaaa bbbbbbb ccccccc");
        let h = height(&t, 12);
        assert_eq!(h, 3, "word wrap must yield three visual rows at width 12");
        let mut terminal = Terminal::new(TestBackend::new(12, 2)).unwrap();
        terminal
            .draw(|frame| {
                t.render(
                    frame,
                    frame.area(),
                    &super::RenderCtx {
                        theme: Theme::dark(),
                        scroll: h - 2,
                        spinner: symbols::SPINNER[0],
                        working: None,
                        queued: &[],
                        hl: &PlainHighlighter,
                    },
                );
            })
            .unwrap();
        assert!(buffer_row(&terminal, 1).contains("ccccccc"));
    }

    #[test]
    fn running_tool_renders_current_spinner_frame() {
        let mut t = Transcript::default();
        t.push_tool(call(1, "Read", "x"));
        let mut terminal = Terminal::new(TestBackend::new(20, 2)).unwrap();
        terminal
            .draw(|frame| {
                t.render(
                    frame,
                    frame.area(),
                    &super::RenderCtx {
                        theme: Theme::dark(),
                        scroll: 0,
                        spinner: symbols::SPINNER[3],
                        working: None,
                        queued: &[],
                        hl: &PlainHighlighter,
                    },
                );
            })
            .unwrap();
        assert!(buffer_row(&terminal, 0).starts_with(symbols::SPINNER[3]));
    }

    #[test]
    fn content_height_exceeds_u16_for_huge_transcripts() {
        let mut t = Transcript::default();
        t.push_user("x\n".repeat(70_000));
        assert!(height(&t, 80) > usize::from(u16::MAX));
    }

    #[test]
    fn queued_rows_render_below_working_and_cap() {
        let mut t = Transcript::default();
        commit(&mut t, "answer");
        let queued: Vec<String> = (0..5).map(|i| format!("queued {i}")).collect();
        let with_queue = t.content_height(80, Theme::dark(), &PlainHighlighter, None, &queued);
        let without = t.content_height(80, Theme::dark(), &PlainHighlighter, None, &[]);
        assert_eq!(with_queue - without, 5, "3 rows + overflow row + spacer");
    }

    #[test]
    fn format_elapsed_scales_units() {
        assert_eq!(format_elapsed(42), "42s");
        assert_eq!(format_elapsed(92), "1m32s");
        assert_eq!(format_elapsed(3_725), "1h02m");
    }

    fn line_text(line: &ratatui::text::Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.clone()).collect()
    }

    #[test]
    fn shell_lifecycle() {
        let mut t = Transcript::default();
        t.push_shell(TaskId(1), "echo hi".to_owned());
        assert!(matches!(
            &t.items[0],
            Item::Shell {
                status: ShellStatus::Running,
                ..
            }
        ));
        t.finish_shell(TaskId(1), "hi".to_owned());
        assert!(matches!(
            &t.items[0],
            Item::Shell {
                status: ShellStatus::Done(output),
                ..
            } if output == "hi"
        ));
    }

    #[test]
    fn complete_interrupted_finishes_running_shell() {
        let mut t = Transcript::default();
        t.push_shell(TaskId(2), "sleep 99".to_owned());
        t.complete(true, &PlainHighlighter, Theme::dark());
        assert!(matches!(
            &t.items[0],
            Item::Shell {
                status: ShellStatus::Done(_),
                ..
            }
        ));
    }

    #[test]
    fn sanitize_strips_ansi_and_resolves_carriage_returns() {
        let lines = sanitize_shell_output("\u{1b}[31mred\u{1b}[0m\nstep1\rstep2\rdone\nlast\r\n\n");
        assert_eq!(lines, vec!["red", "done", "last"]);
    }

    #[test]
    fn shell_rows_render_command_and_output() {
        let rows = shell_rows(
            "echo hi",
            &ShellStatus::Done("hi".to_owned()),
            Theme::dark(),
            80,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(line_text(&rows[0]), "! echo hi");
        assert_eq!(line_text(&rows[1]), "  hi");
    }

    #[test]
    fn shell_rows_caps_visual_rows_and_pins_exit_code() {
        let output = format!("{}\nexit code: 2", "x".repeat(30 * 76));
        let rows = shell_rows("badcmd", &ShellStatus::Done(output), Theme::dark(), 80);
        assert_eq!(rows.len(), 1 + SHELL_BLOCK_CAP + 2);
        assert!(line_text(&rows[1 + SHELL_BLOCK_CAP]).contains("more"));
        assert!(line_text(rows.last().unwrap()).contains("exit code: 2"));
    }

    #[test]
    fn shell_rows_show_no_output_placeholder() {
        let rows = shell_rows(
            "true",
            &ShellStatus::Done("  \n".to_owned()),
            Theme::dark(),
            80,
        );
        assert_eq!(rows.len(), 2);
        assert!(line_text(&rows[1]).contains("(no output)"));
    }
}
