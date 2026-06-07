use std::fmt::Write;

use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
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
        name: "/login",
        desc: "authenticate provider",
    },
    SlashCommand {
        name: "/help",
        desc: "show keybindings and commands",
    },
];

pub struct CommandMenu {
    cursor: usize,
    matches: Vec<usize>,
}

impl CommandMenu {
    pub fn new(prefix: &str) -> Self {
        let matches = Self::compute_matches(prefix);
        Self { cursor: 0, matches }
    }

    fn compute_matches(prefix: &str) -> Vec<usize> {
        let lower = prefix.to_lowercase();
        COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, cmd)| cmd.name.contains(lower.as_str()))
            .map(|(i, _)| i)
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
        self.matches.get(self.cursor).map(|&i| COMMANDS[i].name)
    }

    pub fn desired_height(&self) -> u16 {
        let rows = self.matches.len().max(1);
        u16::try_from(rows).unwrap_or(u16::MAX).saturating_add(2)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        frame.render_widget(Clear, area);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme.border())
            .style(theme.base());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let lines: Vec<Line> = self
            .matches
            .iter()
            .enumerate()
            .map(|(pos, &idx)| {
                let cmd = &COMMANDS[idx];
                let style = if pos == self.cursor {
                    theme.selected()
                } else {
                    theme.base()
                };
                Line::from(vec![
                    Span::styled(format!("  {} ", cmd.name), style),
                    Span::styled(
                        cmd.desc,
                        if pos == self.cursor {
                            style
                        } else {
                            theme.muted()
                        },
                    ),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
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
