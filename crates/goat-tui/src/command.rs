use goat_commands::CommandRegistry;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Span,
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    layout::LIST_MAX,
    overlay::{hint_line, render_window, truncate_to_width},
    symbols,
    theme::Theme,
};

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

fn alias_label(aliases: &[String]) -> String {
    if aliases.is_empty() {
        String::new()
    } else {
        format!(" ({})", aliases.join(", "))
    }
}

struct Match {
    name: String,
    aliases: Vec<String>,
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
                let name_positions = subsequence_match(&query, spec.name);
                let alias_hit = name_positions.is_none()
                    && spec
                        .aliases
                        .iter()
                        .any(|a| subsequence_match(&query, a).is_some());
                (name_positions.is_some() || alias_hit).then(|| Match {
                    name: spec.name.to_owned(),
                    aliases: spec.aliases.iter().map(|a| (*a).to_owned()).collect(),
                    description: spec.description.to_owned(),
                    positions: name_positions.unwrap_or_default(),
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
        let rows = self.matches.len().clamp(1, LIST_MAX);
        u16::try_from(rows).unwrap_or(u16::MAX).saturating_add(1)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let hint_height = 1u16;
        let [list_area, hint_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(hint_height)]).areas(area);

        let width = usize::from(list_area.width);
        let rows = usize::from(list_area.height);
        let lines = render_window(theme, width, self.cursor, self.matches.len(), rows, |idx| {
            let entry = &self.matches[idx];
            let selected = idx == self.cursor;
            let name_style = if selected { theme.key() } else { theme.base() };
            let mut name_spans: Vec<Span> = vec![Span::styled("/", name_style)];
            for (byte_i, ch) in entry.name.char_indices() {
                let style = if entry.positions.contains(&byte_i) {
                    name_style.add_modifier(ratatui::style::Modifier::BOLD)
                } else {
                    name_style
                };
                name_spans.push(Span::styled(ch.to_string(), style));
            }
            if !entry.aliases.is_empty() {
                name_spans.push(Span::styled(alias_label(&entry.aliases), theme.muted()));
            }
            let left_w: usize = name_spans.iter().map(|span| span.content.width()).sum();
            let desc_width = width.saturating_sub(left_w + 6);
            let right = (desc_width > 3).then(|| {
                Span::styled(
                    truncate_to_width(&entry.description, desc_width),
                    theme.muted(),
                )
            });
            (name_spans, right)
        });
        frame.render_widget(Paragraph::new(lines), list_area);

        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::TAB, "complete"),
                    (symbols::key::ENTER, "run"),
                ],
                theme,
            )),
            hint_area,
        );
    }
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use crate::overlay::truncate_to_width;

    #[test]
    fn truncate_short_text_keeps_text() {
        assert_eq!(truncate_to_width("short", 10), "short");
    }

    #[test]
    fn truncate_long_text_fits_width() {
        let truncated = truncate_to_width("very long skill description", 12);
        assert!(truncated.width() <= 12);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn truncate_zero_width_is_empty() {
        assert_eq!(truncate_to_width("text", 0), "");
    }
}
