use std::cell::RefCell;

use goat_protocol::{ToolCall, ToolCallId, ToolDisplay, ToolOutcome};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{highlight::Highlighter, markdown, symbols, theme::Theme};

pub(crate) struct Working {
    pub elapsed: Option<u64>,
    pub label: Option<String>,
    pub thinking: bool,
}

struct RenderCache {
    width: u16,
    version: u64,
    lines: Vec<Line<'static>>,
    rows: Vec<u16>,
    spinner_lines: Vec<usize>,
    total_rows: u16,
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
        let rows: Vec<u16> = lines.iter().map(|l| line_rows(l, width)).collect();
        let total_rows = rows.iter().copied().fold(0u16, u16::saturating_add);
        *self.cache.borrow_mut() = Some(RenderCache {
            width,
            version: self.version,
            lines,
            rows,
            spinner_lines,
            total_rows,
        });
    }

    fn streaming_lines(&self, theme: Theme) -> Vec<Line<'static>> {
        if let Some(buffer) = &self.streaming {
            let mut streamed = labelled(buffer, symbols::marker::AGENT, theme.role_agent(), theme);
            if let Some(last) = streamed.last_mut() {
                last.spans
                    .push(Span::styled(symbols::ui::STREAM_CURSOR, theme.accent()));
            }
            streamed
        } else {
            Vec::new()
        }
    }

    fn working_rows(base: u16, busy: bool) -> u16 {
        if !busy {
            return 0;
        }
        if base > 0 { 2 } else { 1 }
    }

    pub fn content_height(&self, width: u16, theme: Theme, busy: bool) -> u16 {
        self.ensure_cache(theme, width);
        let guard = self.cache.borrow();
        let mut h = guard.as_ref().map_or(0, |c| c.total_rows);
        let streamed = self.streaming_lines(theme);
        if !streamed.is_empty() {
            if h > 0 {
                h = h.saturating_add(1);
            }
            for line in &streamed {
                h = h.saturating_add(line_rows(line, width));
            }
        }
        h.saturating_add(Self::working_rows(h, busy))
    }

    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: Theme,
        scroll: u16,
        spinner: &'static str,
        working: Option<&Working>,
    ) {
        self.ensure_cache(theme, area.width);
        let guard = self.cache.borrow();
        let Some(cache) = guard.as_ref() else {
            return;
        };
        let mut tail: Vec<Line<'static>> = Vec::new();
        let streamed = self.streaming_lines(theme);
        if !streamed.is_empty() && !cache.lines.is_empty() {
            tail.push(Line::default());
        }
        tail.extend(streamed);
        if let Some(w) = working {
            if !cache.lines.is_empty() || !tail.is_empty() {
                tail.push(Line::default());
            }
            tail.push(working_line(theme, spinner, w));
        }

        let start = u32::from(scroll);
        let end = start + u32::from(area.height);
        let mut visible: Vec<Line<'static>> = Vec::new();
        let mut first_offset: u16 = 0;
        let mut cursor: u32 = 0;

        for (i, line) in cache.lines.iter().enumerate() {
            let rows = u32::from(cache.rows[i]);
            if cursor + rows <= start {
                cursor += rows;
                continue;
            }
            if cursor >= end {
                break;
            }
            if visible.is_empty() {
                first_offset = u16::try_from(start.saturating_sub(cursor)).unwrap_or(0);
            }
            let mut line = line.clone();
            if cache.spinner_lines.binary_search(&i).is_ok()
                && let Some(span) = line.spans.first_mut()
            {
                *span = Span::styled(spinner, theme.accent());
            }
            visible.push(line);
            cursor += rows;
        }
        for line in tail {
            let rows = u32::from(line_rows(&line, area.width));
            if cursor + rows <= start {
                cursor += rows;
                continue;
            }
            if cursor >= end {
                break;
            }
            if visible.is_empty() {
                first_offset = u16::try_from(start.saturating_sub(cursor)).unwrap_or(0);
            }
            visible.push(line);
            cursor += rows;
        }

        frame.render_widget(
            Paragraph::new(visible)
                .wrap(Wrap { trim: false })
                .scroll((first_offset, 0)),
            area,
        );
    }
}

fn line_rows(line: &Line<'_>, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }
    let display: usize = line
        .spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    if display == 0 {
        1
    } else {
        u16::try_from(display.div_ceil(usize::from(width))).unwrap_or(u16::MAX)
    }
}

fn working_line(theme: Theme, spinner: &'static str, w: &Working) -> Line<'static> {
    let mut spans = vec![Span::styled(spinner, theme.accent()), Span::raw(" ")];
    let label = w.label.clone().unwrap_or_else(|| {
        let verb = if w.thinking { "thinking" } else { "working" };
        format!("{verb}{}", symbols::ui::ELLIPSIS)
    });
    spans.push(Span::styled(label, theme.muted()));
    if let Some(secs) = w.elapsed {
        spans.push(Span::styled(
            format!("{}{secs}s", symbols::ui::SEPARATOR),
            theme.muted(),
        ));
    }
    Line::from(spans)
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
        lines.extend(item_lines(item, theme, width));
    }
    (lines, spinner_lines)
}

fn item_lines(item: &Item, theme: Theme, width: u16) -> Vec<Line<'static>> {
    match item {
        Item::User(text) => labelled(
            text.as_str(),
            symbols::marker::USER,
            theme.role_user(),
            theme,
        ),
        Item::Agent(rendered) => {
            let mut out: Vec<Line<'_>> = Vec::new();
            if let Some(first) = rendered.first() {
                let mut line = first.clone();
                line.spans
                    .insert(0, Span::styled(symbols::marker::AGENT, theme.role_agent()));
                out.push(line);
            }
            out.extend(rendered.iter().skip(1).map(|l| {
                let mut padded = l.clone();
                padded.spans.insert(0, Span::raw("  "));
                padded
            }));
            out
        }
        Item::Error(text) => labelled(text.as_str(), symbols::marker::ERROR, theme.error(), theme),
        Item::Notice(text) => {
            labelled(text.as_str(), symbols::marker::NOTICE, theme.muted(), theme)
        }
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
                result.extend(result_block(summary, theme));
            }
            result
        }
    }
}

const RESULT_BLOCK_CAP: usize = 6;

fn result_block(summary: &str, theme: Theme) -> Vec<Line<'static>> {
    let lines: Vec<&str> = summary.lines().collect();
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in lines.iter().take(RESULT_BLOCK_CAP) {
        let style = if line.starts_with("+ ") {
            theme.role_agent()
        } else if line.starts_with("- ") {
            theme.error()
        } else {
            theme.muted()
        };
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(line.replace('\t', "  "), style),
        ]));
    }
    if lines.len() > RESULT_BLOCK_CAP {
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "{} {} more",
                    symbols::ui::ELLIPSIS,
                    lines.len() - RESULT_BLOCK_CAP
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

fn labelled(
    text: &str,
    marker: &'static str,
    marker_style: Style,
    theme: Theme,
) -> Vec<Line<'static>> {
    text.split('\n')
        .enumerate()
        .map(|(i, raw)| {
            if i == 0 {
                Line::from(vec![
                    Span::styled(marker, marker_style),
                    Span::styled(raw.to_owned(), theme.base()),
                ])
            } else {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(raw.to_owned(), theme.base()),
                ])
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use goat_protocol::{ToolCall, ToolCallId, ToolOutcome};

    use super::{Item, ToolStatus, Transcript};
    use crate::{highlight::PlainHighlighter, theme::Theme};

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
        let h1 = t.content_height(80, Theme::dark(), false);
        t.push_delta("line one\nline two\nline three\nline four");
        let h2 = t.content_height(80, Theme::dark(), false);
        assert!(
            h2 > h1,
            "content_height must grow while streaming is active"
        );
    }

    #[test]
    fn content_height_includes_working_line() {
        let mut t = Transcript::default();
        commit(&mut t, "hello world");
        let h_idle = t.content_height(80, Theme::dark(), false);
        let h_busy = t.content_height(80, Theme::dark(), true);
        assert!(
            h_busy > h_idle,
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
}
