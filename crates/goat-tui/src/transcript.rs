use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use crate::theme::Theme;

enum Entry {
    User(String),
    Agent(String),
    Tool { name: String, ok: Option<bool> },
    Error(String),
}

#[derive(Default)]
pub struct Transcript {
    entries: Vec<Entry>,
    streaming: Option<String>,
}

impl Transcript {
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.entries.push(Entry::User(text.into()));
    }

    pub fn push_delta(&mut self, chunk: &str) {
        self.streaming
            .get_or_insert_with(String::new)
            .push_str(chunk);
    }

    pub fn finish_agent(&mut self, text: String) {
        self.streaming = None;
        self.entries.push(Entry::Agent(text));
    }

    pub fn push_tool(&mut self, name: String) {
        self.entries.push(Entry::Tool { name, ok: None });
    }

    pub fn finish_tool(&mut self, name: &str, ok: bool) {
        if let Some(Entry::Tool { ok: slot, .. }) = self
            .entries
            .iter_mut()
            .rev()
            .find(|e| matches!(e, Entry::Tool { name: n, ok: None } if n == name))
        {
            *slot = Some(ok);
        }
    }

    pub fn push_error(&mut self, text: impl Into<String>) {
        self.streaming = None;
        self.entries.push(Entry::Error(text.into()));
    }

    pub fn complete(&mut self, interrupted: bool) {
        if let Some(buffer) = self.streaming.take() {
            let text = if interrupted {
                format!("{buffer}  …(interrupted)")
            } else {
                buffer
            };
            self.entries.push(Entry::Agent(text));
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let mut lines: Vec<Line> = Vec::new();
        for entry in &self.entries {
            lines.extend(entry_lines(entry, theme));
            lines.push(Line::default());
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

fn entry_lines(entry: &Entry, theme: Theme) -> Vec<Line<'_>> {
    match entry {
        Entry::User(text) => labelled(text, "› ", theme.role_user(), theme),
        Entry::Agent(text) => labelled(text, "● ", theme.role_agent(), theme),
        Entry::Error(text) => labelled(text, "✗ ", theme.error(), theme),
        Entry::Tool { name, ok } => {
            let (marker, style) = match ok {
                None => ("◐ ", theme.role_tool()),
                Some(true) => ("✓ ", theme.role_tool()),
                Some(false) => ("✗ ", theme.error()),
            };
            vec![Line::from(vec![
                Span::styled(marker, style),
                Span::styled(name.clone(), theme.muted()),
            ])]
        }
    }
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
