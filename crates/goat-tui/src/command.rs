use std::fmt::Write;

use goat_commands::CommandRegistry;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::theme::Theme;

fn subsequence_match(query: &str, target: &str) -> Option<Vec<usize>> {
    if query.is_empty() {
        return Some(Vec::new());
    }
    let mut positions = Vec::new();
    let mut target_chars = target.char_indices();
    for qc in query.chars() {
        loop {
            match target_chars.next() {
                Some((i, tc)) if tc.eq_ignore_ascii_case(&qc) => {
                    positions.push(i);
                    break;
                }
                Some(_) => {}
                None => return None,
            }
        }
    }
    Some(positions)
}

struct Match {
    name: String,
    description: String,
    positions: Vec<usize>,
}

pub struct CommandMenu {
    cursor: usize,
    matches: Vec<Match>,
}

impl CommandMenu {
    pub fn new(registry: &CommandRegistry, prefix: &str) -> Self {
        Self {
            cursor: 0,
            matches: Self::compute_matches(registry, prefix),
        }
    }

    fn compute_matches(registry: &CommandRegistry, prefix: &str) -> Vec<Match> {
        let query = prefix.strip_prefix('/').unwrap_or(prefix).to_lowercase();
        registry
            .specs()
            .into_iter()
            .filter_map(|spec| {
                subsequence_match(&query, spec.name).map(|positions| Match {
                    name: spec.name.to_owned(),
                    description: spec.description.to_owned(),
                    positions,
                })
            })
            .collect()
    }

    pub fn update(&mut self, registry: &CommandRegistry, prefix: &str) {
        self.matches = Self::compute_matches(registry, prefix);
        if self.cursor >= self.matches.len() {
            self.cursor = self.matches.len().saturating_sub(1);
        }
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.matches.len() {
            self.cursor += 1;
        }
    }

    pub fn selected_name(&self) -> Option<String> {
        self.matches.get(self.cursor).map(|m| m.name.clone())
    }

    pub fn desired_height(&self) -> u16 {
        let rows = self.matches.len().max(1);
        u16::try_from(rows).unwrap_or(u16::MAX)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let lines: Vec<Line> = self
            .matches
            .iter()
            .enumerate()
            .map(|(pos, entry)| {
                let selected = pos == self.cursor;
                let name_style = if selected { theme.key() } else { theme.muted() };
                let mut spans: Vec<Span> = vec![Span::styled("  /", name_style)];
                for (byte_i, ch) in entry.name.char_indices() {
                    let style = if entry.positions.contains(&byte_i) {
                        theme.accent()
                    } else {
                        name_style
                    };
                    spans.push(Span::styled(ch.to_string(), style));
                }
                spans.push(Span::styled("   ", theme.base()));
                spans.push(Span::styled(
                    entry.description.clone(),
                    if selected {
                        theme.base()
                    } else {
                        theme.muted()
                    },
                ));
                Line::from(spans)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), area);
    }
}

pub fn help_text(registry: &CommandRegistry) -> String {
    let mut out = String::from("Commands:\n");
    for spec in registry.specs() {
        let _ = writeln!(out, "  /{}  {}", spec.name, spec.description);
    }
    out.push_str("\nKeybindings:\n");
    out.push_str("  Enter       send message\n");
    out.push_str("  Shift/Alt+Enter  newline\n");
    out.push_str("  Ctrl-C      interrupt / quit\n");
    out.push_str("  Ctrl-A/E    line start/end\n");
    out.push_str("  Ctrl-W      delete word before\n");
    out.push_str("  Alt-\u{2190}/\u{2192}     word left/right\n");
    out.push_str("  \u{2191}/\u{2193}         history (when at first/last row)\n");
    out.push_str("  PageUp/Down scroll transcript\n");
    out.push_str("  Tab         complete slash command\n");
    out.push_str("  Esc         clear composer");
    out
}
