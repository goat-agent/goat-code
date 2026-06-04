use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
};
use unicode_normalization::UnicodeNormalization;
use unicode_width::UnicodeWidthChar;

use crate::theme::Theme;

const PROMPT_COLS: u16 = 2;

pub struct Composer {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    history: Vec<String>,
    hist_cursor: Option<usize>,
}

impl Default for Composer {
    fn default() -> Self {
        Self {
            lines: vec![Vec::new()],
            row: 0,
            col: 0,
            history: Vec::new(),
            hist_cursor: None,
        }
    }
}

impl Composer {
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(Vec::is_empty)
    }

    pub fn desired_height(&self) -> u16 {
        let lines = u16::try_from(self.lines.len()).unwrap_or(u16::MAX);
        lines.saturating_add(2).clamp(3, 8)
    }

    pub fn insert_char(&mut self, c: char) {
        self.lines[self.row].insert(self.col, c);
        self.col += 1;
        self.hist_cursor = None;
    }

    pub fn insert_str(&mut self, text: &str) {
        for c in text.nfc() {
            match c {
                '\n' => self.newline(),
                '\r' => {}
                _ => self.insert_char(c),
            }
        }
    }

    pub fn newline(&mut self) {
        let tail = self.lines[self.row].split_off(self.col);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
        self.hist_cursor = None;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.lines[self.row].remove(self.col - 1);
            self.col -= 1;
        } else if self.row > 0 {
            let current = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].extend(current);
        }
        self.hist_cursor = None;
    }

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
    }

    pub fn move_right(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn clear(&mut self) {
        let history = std::mem::take(&mut self.history);
        *self = Self {
            history,
            ..Self::default()
        };
    }

    pub fn take(&mut self) -> String {
        let text = self.text();
        if !text.trim().is_empty() {
            self.history.push(text.clone());
        }
        let history = std::mem::take(&mut self.history);
        *self = Self {
            history,
            ..Self::default()
        };
        text
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.hist_cursor {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.hist_cursor = Some(idx);
        let entry = self.history[idx].clone();
        self.set_text(&entry);
    }

    pub fn history_next(&mut self) {
        match self.hist_cursor {
            Some(i) if i + 1 < self.history.len() => {
                self.hist_cursor = Some(i + 1);
                let entry = self.history[i + 1].clone();
                self.set_text(&entry);
            }
            Some(_) => {
                self.hist_cursor = None;
                self.set_text("");
            }
            None => {}
        }
    }

    fn text(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn set_text(&mut self, text: &str) {
        self.lines = if text.is_empty() {
            vec![Vec::new()]
        } else {
            text.split('\n')
                .map(|line| line.chars().collect())
                .collect()
        };
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
    }

    fn cursor_col(&self) -> u16 {
        let width: usize = self.lines[self.row][..self.col]
            .iter()
            .filter_map(|c| c.width())
            .sum();
        PROMPT_COLS.saturating_add(u16::try_from(width).unwrap_or(u16::MAX))
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme.border())
            .title_bottom(Line::from(Span::styled(" goat-code ", theme.muted())).right_aligned());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let cursor_col = self.cursor_col();
        let scroll = cursor_col.saturating_sub(inner.width.saturating_sub(1));

        let lines: Vec<Line> = self
            .lines
            .iter()
            .enumerate()
            .map(|(i, chars)| {
                let prompt = if i == 0 { "› " } else { "  " };
                let body: String = chars.iter().collect();
                Line::from(vec![
                    Span::styled(prompt, theme.accent()),
                    Span::styled(body, theme.base()),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(lines).scroll((0, scroll)), inner);

        let x = inner.x + cursor_col.saturating_sub(scroll);
        let y = inner.y + u16::try_from(self.row).unwrap_or(u16::MAX);
        frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), y));
    }
}

#[cfg(test)]
mod tests {
    use super::Composer;

    #[test]
    fn cursor_column_counts_wide_chars() {
        let mut composer = Composer::default();
        composer.insert_str("한글");
        assert_eq!(composer.cursor_col(), 2 + 4);
        composer.move_left();
        assert_eq!(composer.cursor_col(), 2 + 2);
    }

    #[test]
    fn paste_normalizes_to_nfc() {
        let mut composer = Composer::default();
        composer.insert_str("\u{1100}\u{1161}");
        assert_eq!(composer.cursor_col(), 2 + 2);
    }
}
