use std::cell::RefCell;

use goat_protocol::{ToolCall, ToolCallId, ToolDisplay, ToolOutcome};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{highlight::Highlighter, markdown, symbols, theme::Theme, wrap};

pub(crate) struct Working {
    pub elapsed: Option<u64>,
    pub label: Option<String>,
    pub thinking: bool,
}

pub(crate) struct RenderCtx<'a> {
    pub theme: Theme,
    pub scroll: usize,
    pub spinner: &'static str,
    pub working: Option<&'a Working>,
}

struct RenderCache {
    width: u16,
    version: u64,
    lines: Vec<Line<'static>>,
    spinner_lines: Vec<usize>,
}

#[derive(Debug)]
pub(crate) enum ToolStatus {
    Running,
    Done(ToolOutcome),
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
    Error(String),
    Notice(String),
}

#[derive(Default)]
pub struct Transcript {
    pub(crate) items: Vec<Item>,
    streaming: Option<String>,
    version: u64,
    cache: RefCell<Option<RenderCache>>,
}

impl Transcript {
    fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
        *self.cache.borrow_mut() = None;
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

    pub fn push_error(&mut self, text: impl Into<String>) {
        self.bump_version();
        self.streaming = None;
        self.items.push(Item::Error(text.into()));
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
        } else if interrupted {
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

    fn streaming_rows(&self, theme: Theme, width: u16) -> Vec<Line<'static>> {
        let Some(buffer) = &self.streaming else {
            return Vec::new();
        };
        let mut content = plain_lines(buffer, theme);
        if let Some(last) = content.last_mut() {
            last.spans
                .push(Span::styled(symbols::ui::STREAM_CURSOR, theme.accent()));
        }
        hang(
            &content,
            Span::styled(symbols::marker::AGENT, theme.role_agent()),
            width,
        )
    }

    fn tail_rows(
        &self,
        theme: Theme,
        width: u16,
        spinner: &'static str,
        working: Option<&Working>,
        base_nonempty: bool,
    ) -> Vec<Line<'static>> {
        let mut tail: Vec<Line<'static>> = Vec::new();
        let streamed = self.streaming_rows(theme, width);
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
        tail
    }

    pub fn content_height(&self, width: u16, theme: Theme, working: Option<&Working>) -> usize {
        self.ensure_cache(theme, width);
        let base = self.cache.borrow().as_ref().map_or(0, |c| c.lines.len());
        base + self
            .tail_rows(theme, width, symbols::SPINNER[0], working, base > 0)
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
            ctx.spinner,
            ctx.working,
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
            format!("{}{secs}s", symbols::ui::SEPARATOR),
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
                Span::styled(name.clone(), theme.tool_name()),
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
    use goat_protocol::{ToolCall, ToolCallId, ToolOutcome};
    use ratatui::{Terminal, backend::TestBackend};

    use super::{Item, ToolStatus, Transcript, Working};
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
        t.content_height(width, Theme::dark(), None)
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
        };
        let busy = t.content_height(80, Theme::dark(), Some(&working));
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
}
