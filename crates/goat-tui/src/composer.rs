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
const PLACEHOLDER: &str = "Ask anything…";

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

    pub fn on_first_row(&self) -> bool {
        self.row == 0
    }

    pub fn on_last_row(&self) -> bool {
        self.row + 1 == self.lines.len()
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

    pub fn delete_forward(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.lines[self.row].remove(self.col);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].extend(next);
        }
        self.hist_cursor = None;
    }

    pub fn delete_word_before(&mut self) {
        while self.col > 0 && self.lines[self.row][self.col - 1] == ' ' {
            self.lines[self.row].remove(self.col - 1);
            self.col -= 1;
        }
        while self.col > 0 && self.lines[self.row][self.col - 1] != ' ' {
            self.lines[self.row].remove(self.col - 1);
            self.col -= 1;
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

    pub fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].len());
        }
    }

    pub fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].len());
        }
    }

    pub fn move_home(&mut self) {
        self.col = 0;
    }

    pub fn move_end(&mut self) {
        self.col = self.lines[self.row].len();
    }

    pub fn move_word_left(&mut self) {
        while self.col > 0 && self.lines[self.row][self.col - 1] == ' ' {
            self.col -= 1;
        }
        while self.col > 0 && self.lines[self.row][self.col - 1] != ' ' {
            self.col -= 1;
        }
    }

    pub fn move_word_right(&mut self) {
        let len = self.lines[self.row].len();
        while self.col < len && self.lines[self.row][self.col] != ' ' {
            self.col += 1;
        }
        while self.col < len && self.lines[self.row][self.col] == ' ' {
            self.col += 1;
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

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme, focused: bool) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme.border())
            .title_bottom(Line::from(Span::styled(" goat-code ", theme.muted())).right_aligned());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("› ", theme.accent()),
                    Span::styled(PLACEHOLDER, theme.muted()),
                ])),
                inner,
            );
            if focused {
                let x = inner.x + PROMPT_COLS;
                frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), inner.y));
            }
            return;
        }

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

        if focused {
            let x = inner.x + cursor_col.saturating_sub(scroll);
            let y = inner.y + u16::try_from(self.row).unwrap_or(u16::MAX);
            frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), y));
        }
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

    #[test]
    fn delete_forward_removes_next_char() {
        let mut composer = Composer::default();
        composer.insert_str("abc");
        composer.move_home();
        composer.delete_forward();
        assert_eq!(composer.text(), "bc");
    }

    #[test]
    fn delete_word_before_removes_last_word() {
        let mut composer = Composer::default();
        composer.insert_str("hello world");
        composer.delete_word_before();
        assert_eq!(composer.text(), "hello ");
    }

    #[test]
    fn move_up_down_stays_within_bounds() {
        let mut composer = Composer::default();
        composer.insert_str("line1\nline2\nline3");
        assert_eq!(composer.row, 2);
        composer.move_up();
        assert_eq!(composer.row, 1);
        composer.move_down();
        assert_eq!(composer.row, 2);
        composer.move_down();
        assert_eq!(composer.row, 2);
    }

    #[test]
    fn home_end_move_to_line_bounds() {
        let mut composer = Composer::default();
        composer.insert_str("hello");
        composer.move_home();
        assert_eq!(composer.col, 0);
        composer.move_end();
        assert_eq!(composer.col, 5);
    }

    #[test]
    fn on_first_last_row_correct() {
        let mut composer = Composer::default();
        assert!(composer.on_first_row());
        assert!(composer.on_last_row());
        composer.insert_str("a\nb");
        assert!(!composer.on_first_row());
        assert!(composer.on_last_row());
    }

    #[test]
    fn placeholder_shows_when_empty() {
        let composer = Composer::default();
        assert!(composer.is_empty());
    }
}
