use goat_protocol::{ToolCall, ToolCallId, ToolOutcome};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Theme;

#[derive(Debug)]
enum ToolStatus {
    Running,
    Done(ToolOutcome),
}

#[derive(Debug)]
enum Item {
    User(String),
    Text(String),
    Tool {
        id: ToolCallId,
        name: String,
        input: String,
        status: ToolStatus,
    },
    Error(String),
}

#[derive(Default)]
pub struct Transcript {
    items: Vec<Item>,
    streaming: Option<String>,
}

impl Transcript {
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.items.push(Item::User(text.into()));
    }

    pub fn push_delta(&mut self, chunk: &str) {
        self.streaming
            .get_or_insert_with(String::new)
            .push_str(chunk);
    }

    pub fn commit_text(&mut self, text: String) {
        self.streaming = None;
        self.items.push(Item::Text(text));
    }

    pub fn push_tool(&mut self, call: ToolCall) {
        self.items.push(Item::Tool {
            id: call.id,
            name: call.name,
            input: call.input,
            status: ToolStatus::Running,
        });
    }

    pub fn finish_tool(&mut self, call_id: ToolCallId, outcome: ToolOutcome) {
        for item in self.items.iter_mut().rev() {
            if let Item::Tool { id, status, .. } = item {
                if *id == call_id && matches!(status, ToolStatus::Running) {
                    *status = ToolStatus::Done(outcome);
                    return;
                }
            }
        }
    }

    pub fn push_error(&mut self, text: impl Into<String>) {
        self.streaming = None;
        self.items.push(Item::Error(text.into()));
    }

    pub fn complete(&mut self, interrupted: bool) {
        if interrupted {
            for item in &mut self.items {
                if let Item::Tool { status, .. } = item {
                    if matches!(status, ToolStatus::Running) {
                        *status = ToolStatus::Done(ToolOutcome {
                            ok: false,
                            summary: None,
                        });
                    }
                }
            }
        }
        if let Some(buffer) = self.streaming.take() {
            let text = if interrupted {
                format!("{buffer}  …(interrupted)")
            } else {
                buffer
            };
            self.items.push(Item::Text(text));
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let mut lines: Vec<Line> = Vec::new();

        for (i, item) in self.items.iter().enumerate() {
            lines.extend(item_lines(item, theme, area.width));
            let next_is_tool = matches!(self.items.get(i + 1), Some(Item::Tool { .. }));
            if !(matches!(item, Item::Tool { .. }) && next_is_tool) {
                lines.push(Line::default());
            }
        }

        if let Some(buffer) = &self.streaming {
            let mut streamed = labelled(buffer, "● ", theme.role_agent(), theme);
            if let Some(last) = streamed.last_mut() {
                last.spans.push(Span::styled("▌", theme.accent()));
            }
            lines.extend(streamed);
        }

        let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let scroll = total.saturating_sub(area.height);
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0)),
            area,
        );
    }
}

fn item_lines(item: &Item, theme: Theme, width: u16) -> Vec<Line<'_>> {
    match item {
        Item::User(text) => labelled(text, "› ", theme.role_user(), theme),
        Item::Text(text) => labelled(text, "● ", theme.role_agent(), theme),
        Item::Error(text) => labelled(text, "✗ ", theme.error(), theme),
        Item::Tool {
            name,
            input,
            status,
            ..
        } => {
            let (marker, marker_style) = match status {
                ToolStatus::Running => ("◐ ", theme.accent()),
                ToolStatus::Done(ToolOutcome { ok: true, .. }) => ("✓ ", theme.role_tool()),
                ToolStatus::Done(ToolOutcome { ok: false, .. }) => ("✗ ", theme.error()),
            };

            let summary = match status {
                ToolStatus::Done(ToolOutcome {
                    summary: Some(s), ..
                }) => Some(s.as_str()),
                _ => None,
            };

            let name_w = name.width();
            let summary_w = summary.map_or(0, |s| s.width() + 2);
            let avail = usize::from(width)
                .saturating_sub(2)
                .saturating_sub(name_w)
                .saturating_sub(1)
                .saturating_sub(summary_w);

            let shown_input = truncate_to_width(input, avail);

            let mut spans = vec![
                Span::styled(marker, marker_style),
                Span::styled(name.clone(), theme.base()),
                Span::raw(" "),
                Span::styled(shown_input, theme.muted()),
            ];
            if let Some(s) = summary {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(s.to_owned(), theme.muted()));
            }

            let mut result = vec![Line::from(spans)];

            if let ToolStatus::Done(ToolOutcome {
                ok: false,
                summary: Some(detail),
            }) = status
            {
                for line in detail.lines().take(2) {
                    result.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(line.to_owned(), theme.muted()),
                    ]));
                }
            }

            result
        }
    }
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

fn labelled<'a>(
    text: &'a str,
    marker: &'static str,
    marker_style: Style,
    theme: Theme,
) -> Vec<Line<'a>> {
    text.split('\n')
        .enumerate()
        .map(|(i, raw)| {
            if i == 0 {
                Line::from(vec![
                    Span::styled(marker, marker_style),
                    Span::styled(raw, theme.base()),
                ])
            } else {
                Line::from(vec![Span::raw("  "), Span::styled(raw, theme.base())])
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use goat_protocol::{ToolCall, ToolCallId, ToolOutcome};

    use super::{Item, ToolStatus, Transcript};

    fn call(id: u64, name: &str, input: &str) -> ToolCall {
        ToolCall {
            id: ToolCallId(id),
            name: name.to_owned(),
            input: input.to_owned(),
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
        t.commit_text("step one".into());
        t.push_tool(call(1, "Read", "src/lib.rs"));
        t.finish_tool(ToolCallId(1), ok());
        t.push_delta("step two");
        t.commit_text("step two".into());

        assert!(matches!(&t.items[0], Item::User(_)));
        assert!(matches!(&t.items[1], Item::Text(s) if s == "step one"));
        assert!(matches!(&t.items[2], Item::Tool { .. }));
        assert!(matches!(&t.items[3], Item::Text(s) if s == "step two"));
    }

    #[test]
    fn complete_interrupted_clears_running_tools() {
        let mut t = Transcript::default();
        t.push_tool(call(5, "Bash", "long cmd"));
        t.complete(true);
        if let Some(Item::Tool {
            status: ToolStatus::Done(o),
            ..
        }) = t.items.last()
        {
            assert!(!o.ok);
        } else {
            panic!("expected failed tool");
        }
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
}
