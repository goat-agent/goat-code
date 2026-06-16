use std::path::{Path, PathBuf};

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{layout::LIST_MAX, overlay::render_window, theme::Theme};

const SCAN_CAP: usize = 4000;
const RESULT_CAP: usize = 200;

pub struct FileMenu {
    entries: Vec<String>,
    matches: Vec<String>,
    cursor: usize,
}

impl FileMenu {
    pub fn new(root: &Path, query: &str) -> Self {
        let entries = scan(root);
        let mut menu = Self {
            entries,
            matches: Vec::new(),
            cursor: 0,
        };
        menu.refilter(query);
        menu
    }

    pub fn update(&mut self, query: &str) {
        self.refilter(query);
    }

    fn refilter(&mut self, query: &str) {
        let needle = query.to_lowercase();
        self.matches = self
            .entries
            .iter()
            .filter(|e| needle.is_empty() || e.to_lowercase().contains(&needle))
            .take(RESULT_CAP)
            .cloned()
            .collect();
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

    pub fn selected(&self) -> Option<String> {
        self.matches.get(self.cursor).cloned()
    }

    pub fn desired_height(&self) -> u16 {
        let rows = self.matches.len().clamp(1, LIST_MAX);
        u16::try_from(rows).unwrap_or(u16::MAX)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let [list_area] = Layout::vertical([Constraint::Min(1)]).areas(area);
        let width = usize::from(list_area.width);
        let rows = usize::from(list_area.height);
        let lines = if self.matches.is_empty() {
            vec![Line::from(Span::styled(" no files match", theme.muted()))]
        } else {
            render_window(theme, width, self.cursor, self.matches.len(), rows, |idx| {
                let entry = &self.matches[idx];
                let selected = idx == self.cursor;
                let style = if selected { theme.key() } else { theme.base() };
                (vec![Span::styled(entry.clone(), style)], None)
            })
        };
        frame.render_widget(Paragraph::new(lines), list_area);
    }
}

fn scan(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= SCAN_CAP {
            break;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name.as_ref() == "target" || name.as_ref() == "node_modules"
            {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if is_dir {
                out.push(format!("{rel}/"));
                stack.push(path);
            } else {
                out.push(rel);
            }
            if out.len() >= SCAN_CAP {
                break;
            }
        }
    }
    out.sort();
    out
}
