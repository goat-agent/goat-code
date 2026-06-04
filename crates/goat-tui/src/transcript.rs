use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

use crate::{highlight::Highlighter, markdown, theme::Theme};

enum Entry {
    User(String),
    Agent(Vec<Line<'static>>),
    Tool {
        name: String,
        input: Option<String>,
        ok: Option<bool>,
    },
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

    pub fn finish_agent(&mut self, text: &str, hl: &dyn Highlighter, theme: &Theme) {
        self.streaming = None;
        self.entries
            .push(Entry::Agent(markdown::render(text, *theme, hl)));
    }

    pub fn push_tool(&mut self, name: String, input: Option<String>) {
        self.entries.push(Entry::Tool {
            name,
            input,
            ok: None,
        });
    }

    pub fn finish_tool(&mut self, name: &str, ok: bool) {
        if let Some(Entry::Tool { ok: slot, .. }) = self
            .entries
            .iter_mut()
            .rev()
            .find(|e| matches!(e, Entry::Tool { name: n, ok: None, .. } if n == name))
        {
            *slot = Some(ok);
        }
    }

    pub fn push_error(&mut self, text: impl Into<String>) {
        self.streaming = None;
        self.entries.push(Entry::Error(text.into()));
    }

    pub fn complete(&mut self, interrupted: bool, hl: &dyn Highlighter, theme: &Theme) {
        if let Some(buffer) = self.streaming.take() {
            let text = if interrupted {
                format!("{buffer}  …(interrupted)")
            } else {
                buffer
            };
            self.entries
                .push(Entry::Agent(markdown::render(&text, *theme, hl)));
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme, scroll: u16) {
        let mut lines: Vec<Line> = Vec::new();
        for entry in &self.entries {
            lines.extend(entry_lines(entry, theme));
            lines.push(Line::default());
        }
        if let Some(buffer) = &self.streaming {
            let mut streamed = labelled_plain(buffer, "● ", theme.role_agent(), theme);
            if let Some(last) = streamed.last_mut() {
                last.spans.push(Span::styled("▌", theme.accent()));
            }
            lines.extend(streamed);
        }

        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(para.scroll((scroll, 0)), area);
    }

    pub fn content_height(&self, width: u16, theme: Theme) -> u16 {
        let mut lines: Vec<Line> = Vec::new();
        for entry in &self.entries {
            lines.extend(entry_lines(entry, theme));
            lines.push(Line::default());
        }
        if let Some(buffer) = &self.streaming {
            lines.extend(labelled_plain(buffer, "● ", theme.role_agent(), theme));
        }
        if width == 0 {
            return u16::try_from(lines.len()).unwrap_or(u16::MAX);
        }
        let w = usize::from(width);
        lines
            .iter()
            .map(|l| {
                let display: usize = l
                    .spans
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                    .sum();
                let rows = if display == 0 { 1 } else { display.div_ceil(w) };
                u16::try_from(rows).unwrap_or(u16::MAX)
            })
            .sum::<u16>()
    }
}

fn entry_lines<'a>(entry: &'a Entry, theme: Theme) -> Vec<Line<'a>> {
    match entry {
        Entry::User(text) => labelled_plain(text, "› ", theme.role_user(), theme),
        Entry::Agent(rendered) => {
            let mut out: Vec<Line<'a>> = Vec::new();
            if let Some(first) = rendered.first() {
                let mut marker_line = first.clone();
                marker_line
                    .spans
                    .insert(0, Span::styled("● ", theme.role_agent()));
                out.push(marker_line);
            }
            if rendered.len() > 1 {
                out.extend(rendered[1..].iter().map(|l| {
                    let mut padded = l.clone();
                    padded.spans.insert(0, Span::raw("  "));
                    padded
                }));
            }
            out
        }
        Entry::Error(text) => labelled_plain(text, "✗ ", theme.error(), theme),
        Entry::Tool { name, input, ok } => {
            let (marker, style) = match ok {
                None => ("◐ ", theme.role_tool()),
                Some(true) => ("✓ ", theme.role_tool()),
                Some(false) => ("✗ ", theme.error()),
            };
            let mut spans = vec![
                Span::styled(marker, style),
                Span::styled(name.clone(), theme.base()),
            ];
            if let Some(arg) = input {
                spans.push(Span::styled(format!(" {arg}"), theme.muted()));
            }
            vec![Line::from(spans)]
        }
    }
}

fn labelled_plain<'a>(
    text: &'a str,
    marker: &'static str,
    marker_style: ratatui::style::Style,
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
