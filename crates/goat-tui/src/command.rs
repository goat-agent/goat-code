use std::fmt::Write;

use goat_commands::CommandRegistry;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{overlay::selection_row, symbols, theme::Theme};

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
        let rows = self.matches.len().max(1);
        u16::try_from(rows).unwrap_or(u16::MAX).saturating_add(1)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let hint_height = 1u16;
        let [list_area, hint_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(hint_height)]).areas(area);

        let width = usize::from(list_area.width);
        let lines: Vec<Line> = self
            .matches
            .iter()
            .enumerate()
            .map(|(pos, entry)| {
                let selected = pos == self.cursor;
                let name_style = if selected { theme.key() } else { theme.muted() };
                let mut name_spans: Vec<Span> = vec![Span::styled("/", name_style)];
                for (byte_i, ch) in entry.name.char_indices() {
                    let style = if entry.positions.contains(&byte_i) {
                        theme.accent()
                    } else {
                        name_style
                    };
                    name_spans.push(Span::styled(ch.to_string(), style));
                }
                if !entry.aliases.is_empty() {
                    name_spans.push(Span::styled(alias_label(&entry.aliases), theme.muted()));
                }
                let desc_style = if selected {
                    theme.base()
                } else {
                    theme.muted()
                };
                let right = Some(Span::styled(entry.description.clone(), desc_style));
                selection_row(theme, selected, width, name_spans, right)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), list_area);

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(
                    " {}{} navigate{}{}  complete{}  run  esc close",
                    symbols::key::ARROW_UP,
                    symbols::key::ARROW_DOWN,
                    symbols::ui::SEPARATOR,
                    symbols::key::TAB,
                    symbols::ui::SEPARATOR,
                ),
                theme.muted(),
            ))),
            hint_area,
        );
    }
}

pub fn help_text(registry: &CommandRegistry) -> String {
    let mut out = String::from("Commands:\n");
    for spec in registry.specs() {
        let label = if spec.aliases.is_empty() {
            String::new()
        } else {
            format!(" ({})", spec.aliases.join(", "))
        };
        let _ = writeln!(out, "  /{}{}  {}", spec.name, label, spec.description);
    }
    out.push_str("\nKeybindings:\n");
    out.push_str("  Enter            send message\n");
    out.push_str("  Shift/Alt+Enter  newline\n");
    out.push_str("  Ctrl-C           interrupt / quit\n");
    out.push_str("  Ctrl-A/E         line start/end\n");
    out.push_str("  Ctrl-W           delete word before\n");
    let _ = writeln!(
        out,
        "  Alt-{}/{}          word left/right",
        symbols::key::ARROW_LEFT,
        symbols::key::ARROW_RIGHT
    );
    let _ = writeln!(
        out,
        "  {}/{}              history (when at first/last row)",
        symbols::key::ARROW_UP,
        symbols::key::ARROW_DOWN
    );
    out.push_str("  PageUp/Down      scroll transcript\n");
    let _ = writeln!(
        out,
        "  {}               complete slash command",
        symbols::key::TAB
    );
    out.push_str("  Esc              clear composer");
    out
}
