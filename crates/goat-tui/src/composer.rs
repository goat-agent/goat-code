use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
};
use unicode_normalization::UnicodeNormalization;
use unicode_width::UnicodeWidthChar;

use crate::{symbols, theme::Theme, wrap};

const PROMPT_COLS: u16 = 2;
const BORDER_COLS: u16 = 2;
const PLACEHOLDER: &str = "Ask anything…";

pub struct Composer {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    history: Vec<String>,
    hist_cursor: Option<usize>,
    draft: Option<String>,
}

impl Default for Composer {
    fn default() -> Self {
        Self {
            lines: vec![Vec::new()],
            row: 0,
            col: 0,
            history: Vec::new(),
            hist_cursor: None,
            draft: None,
        }
    }
}

fn word_boundary(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Composer {
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(Vec::is_empty)
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        let wrap_width = width.saturating_sub(PROMPT_COLS + BORDER_COLS).max(1);
        let total: usize = self
            .lines
            .iter()
            .map(|line| wrap::wrap_chars(line, wrap_width).len())
            .sum();
        u16::try_from(total)
            .unwrap_or(u16::MAX)
            .saturating_add(2)
            .clamp(3, 8)
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
        while self.col > 0 && !word_boundary(self.lines[self.row][self.col - 1]) {
            self.lines[self.row].remove(self.col - 1);
            self.col -= 1;
        }
        while self.col > 0 && word_boundary(self.lines[self.row][self.col - 1]) {
            self.lines[self.row].remove(self.col - 1);
            self.col -= 1;
        }
        self.hist_cursor = None;
    }

    pub fn move_left(&mut self) -> bool {
        if self.col > 0 {
            self.col -= 1;
            true
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
            true
        } else {
            false
        }
    }

    pub fn move_right(&mut self) -> bool {
        if self.col < self.lines[self.row].len() {
            self.col += 1;
            true
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
            true
        } else {
            false
        }
    }

    pub fn move_up(&mut self) -> bool {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].len());
            true
        } else {
            false
        }
    }

    pub fn move_down(&mut self) -> bool {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].len());
            true
        } else {
            false
        }
    }

    pub fn move_home(&mut self) -> bool {
        if self.col != 0 {
            self.col = 0;
            true
        } else {
            false
        }
    }

    pub fn move_end(&mut self) -> bool {
        let end = self.lines[self.row].len();
        if self.col == end {
            false
        } else {
            self.col = end;
            true
        }
    }

    pub fn move_word_left(&mut self) -> bool {
        let start_col = self.col;
        let start_row = self.row;
        while self.col > 0 && !word_boundary(self.lines[self.row][self.col - 1]) {
            self.col -= 1;
        }
        while self.col > 0 && word_boundary(self.lines[self.row][self.col - 1]) {
            self.col -= 1;
        }
        self.col != start_col || self.row != start_row
    }

    pub fn move_word_right(&mut self) -> bool {
        let start_col = self.col;
        let start_row = self.row;
        let len = self.lines[self.row].len();
        while self.col < len && !word_boundary(self.lines[self.row][self.col]) {
            self.col += 1;
        }
        while self.col < len && word_boundary(self.lines[self.row][self.col]) {
            self.col += 1;
        }
        self.col != start_col || self.row != start_row
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

    pub fn discard(&mut self) {
        self.take();
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.hist_cursor {
            None => {
                self.draft = Some(self.text());
                self.history.len() - 1
            }
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
                let draft = self.draft.take().unwrap_or_default();
                self.set_text(&draft);
            }
            None => {}
        }
    }

    pub(crate) fn text(&self) -> String {
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

    fn visual_cursor(&self, wrap_width: u16) -> (usize, u16) {
        let mut row = 0usize;
        for line in &self.lines[..self.row] {
            row += wrap::wrap_chars(line, wrap_width).len();
        }
        let ranges = wrap::wrap_chars(&self.lines[self.row], wrap_width);
        let idx = ranges
            .iter()
            .position(|r| self.col < r.end)
            .unwrap_or(ranges.len() - 1);
        let range = ranges[idx].clone();
        let col: usize = self.lines[self.row][range.start..self.col.max(range.start)]
            .iter()
            .filter_map(|c| c.width())
            .sum();
        (row + idx, u16::try_from(col).unwrap_or(u16::MAX))
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme, focused: bool) {
        let border = if focused {
            theme.border()
        } else {
            theme.border_dim()
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(symbols::marker::USER, theme.accent()),
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

        let wrap_width = inner.width.saturating_sub(PROMPT_COLS).max(1);
        let mut rows: Vec<Line> = Vec::new();
        for chars in &self.lines {
            for range in wrap::wrap_chars(chars, wrap_width) {
                let prompt = if rows.is_empty() {
                    symbols::marker::USER
                } else {
                    "  "
                };
                let body: String = chars[range].iter().collect();
                rows.push(Line::from(vec![
                    Span::styled(prompt, theme.accent()),
                    Span::styled(body, theme.base()),
                ]));
            }
        }

        let (cursor_row, cursor_col) = self.visual_cursor(wrap_width);
        let visible_rows = usize::from(inner.height).max(1);
        let offset = cursor_row.saturating_sub(visible_rows - 1);
        let visible: Vec<Line> = rows.into_iter().skip(offset).take(visible_rows).collect();
        frame.render_widget(Paragraph::new(visible), inner);

        if focused {
            let x = inner.x + PROMPT_COLS + cursor_col;
            let y = inner.y + u16::try_from(cursor_row - offset).unwrap_or(u16::MAX);
            frame.set_cursor_position((
                x.min(inner.right().saturating_sub(1)),
                y.min(inner.bottom().saturating_sub(1)),
            ));
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
        assert_eq!(composer.visual_cursor(80), (0, 4));
        composer.move_left();
        assert_eq!(composer.visual_cursor(80), (0, 2));
    }

    #[test]
    fn paste_normalizes_to_nfc() {
        let mut composer = Composer::default();
        composer.insert_str("\u{1100}\u{1161}");
        assert_eq!(composer.visual_cursor(80), (0, 2));
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
        let changed = composer.move_down();
        assert!(!changed);
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

    #[test]
    fn move_returns_false_at_boundary() {
        let mut composer = Composer::default();
        assert!(!composer.move_left());
        assert!(!composer.move_right());
        assert!(!composer.move_up());
        assert!(!composer.move_down());
        assert!(!composer.move_home());
        assert!(!composer.move_end());
    }

    #[test]
    fn clear_does_not_pollute_history() {
        let mut composer = Composer::default();
        composer.insert_str("some draft");
        composer.clear();
        assert!(composer.is_empty());
        composer.history_prev();
        assert!(composer.is_empty());
    }

    #[test]
    fn discard_preserves_draft_in_history() {
        let mut composer = Composer::default();
        composer.insert_str("important draft");
        composer.discard();
        assert!(composer.is_empty());
        composer.history_prev();
        assert_eq!(composer.text(), "important draft");
    }

    #[test]
    fn history_navigation_restores_draft() {
        let mut composer = Composer::default();
        composer.insert_str("sent");
        composer.take();
        composer.insert_str("work in progress");
        composer.history_prev();
        assert_eq!(composer.text(), "sent");
        composer.history_next();
        assert_eq!(composer.text(), "work in progress");
    }

    #[test]
    fn word_boundary_movement_skips_non_alnum() {
        let mut composer = Composer::default();
        composer.insert_str("hello_world foo");
        composer.move_word_left();
        assert_eq!(composer.col, 12);
        composer.move_word_left();
        assert_eq!(composer.col, 0);
    }

    #[test]
    fn desired_height_wraps_long_line() {
        let mut composer = Composer::default();
        let long: String = "a".repeat(40);
        composer.insert_str(&long);
        let h_narrow = composer.desired_height(24);
        let h_wide = composer.desired_height(80);
        assert!(h_narrow > h_wide);
    }

    #[test]
    fn desired_height_counts_wide_chars_exactly() {
        let mut composer = Composer::default();
        composer.insert_str("한한한한한");
        assert_eq!(composer.desired_height(9), 5);
    }

    #[test]
    fn visual_cursor_row_tracks_wrapping() {
        let mut composer = Composer::default();
        composer.insert_str("abcdefghij");
        assert_eq!(composer.visual_cursor(4), (2, 2));
    }
}
