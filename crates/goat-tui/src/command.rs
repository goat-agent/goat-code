use std::fmt::Write;

use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::theme::Theme;

pub struct SlashCommand {
    pub name: &'static str,
    pub desc: &'static str,
}

pub static COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "/model",
        desc: "switch model",
    },
    SlashCommand {
        name: "/config",
        desc: "configure providers and settings",
    },
    SlashCommand {
        name: "/clear",
        desc: "start a new conversation",
    },
    SlashCommand {
        name: "/help",
        desc: "show keybindings and commands",
    },
];

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

pub struct CommandMenu {
    cursor: usize,
    matches: Vec<(usize, Vec<usize>)>,
}

impl CommandMenu {
    pub fn new(prefix: &str) -> Self {
        let matches = Self::compute_matches(prefix);
        Self { cursor: 0, matches }
    }

    fn compute_matches(prefix: &str) -> Vec<(usize, Vec<usize>)> {
        let query = prefix.strip_prefix('/').unwrap_or(prefix).to_lowercase();
        COMMANDS
            .iter()
            .enumerate()
            .filter_map(|(i, cmd)| {
                let name = cmd.name.strip_prefix('/').unwrap_or(cmd.name);
                subsequence_match(&query, name).map(|positions| (i, positions))
            })
            .collect()
    }

    pub fn update(&mut self, prefix: &str) {
        self.matches = Self::compute_matches(prefix);
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

    pub fn selected_name(&self) -> Option<&'static str> {
        self.matches
            .get(self.cursor)
            .map(|&(i, _)| COMMANDS[i].name)
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
            .map(|(pos, &(idx, ref positions))| {
                let cmd = &COMMANDS[idx];
                let selected = pos == self.cursor;
                let name_no_slash = cmd.name.strip_prefix('/').unwrap_or(cmd.name);
                let name_style = if selected { theme.key() } else { theme.muted() };
                let mut spans: Vec<Span> = vec![Span::styled("  /", name_style)];
                for (byte_i, ch) in name_no_slash.char_indices() {
                    let style = if positions.contains(&byte_i) {
                        theme.accent()
                    } else {
                        name_style
                    };
                    spans.push(Span::styled(ch.to_string(), style));
                }
                spans.push(Span::styled("   ", theme.base()));
                spans.push(Span::styled(
                    cmd.desc,
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

pub fn help_text() -> String {
    let mut out = String::from("Commands:\n");
    for cmd in COMMANDS {
        let _ = writeln!(out, "  {}  {}", cmd.name, cmd.desc);
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
