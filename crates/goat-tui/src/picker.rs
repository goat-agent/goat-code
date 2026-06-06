use goat_protocol::{AccountChoice, ModelEntry, ModelTarget};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::theme::Theme;

pub enum PickerOutcome {
    Pending,
    Selected(ModelTarget),
}

struct AccountPicker {
    choices: Vec<AccountChoice>,
    cursor: usize,
}

pub struct Picker {
    entries: Vec<ModelEntry>,
    query: String,
    matches: Vec<ModelEntry>,
    cursor: usize,
    account: Option<AccountPicker>,
}

impl Picker {
    pub fn new(entries: Vec<ModelEntry>) -> Self {
        let mut picker = Self {
            entries,
            query: String::new(),
            matches: Vec::new(),
            cursor: 0,
            account: None,
        };
        picker.refilter();
        picker
    }

    pub fn set_entries(&mut self, entries: Vec<ModelEntry>) {
        self.entries = entries;
        self.refilter();
    }

    fn refilter(&mut self) {
        let needle = self.query.to_lowercase();
        self.matches = self
            .entries
            .iter()
            .filter(|entry| {
                format!("{}/{}", entry.provider, entry.model)
                    .to_lowercase()
                    .contains(&needle)
            })
            .cloned()
            .collect();
        if self.cursor >= self.matches.len() {
            self.cursor = self.matches.len().saturating_sub(1);
        }
    }

    pub fn on_char(&mut self, ch: char) {
        if self.account.is_some() {
            return;
        }
        self.query.push(ch);
        self.refilter();
    }

    pub fn backspace(&mut self) {
        if self.account.is_some() {
            return;
        }
        self.query.pop();
        self.refilter();
    }

    pub fn move_up(&mut self) {
        if let Some(account) = &mut self.account {
            account.cursor = account.cursor.saturating_sub(1);
        } else {
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    pub fn move_down(&mut self) {
        if let Some(account) = &mut self.account {
            if account.cursor + 1 < account.choices.len() {
                account.cursor += 1;
            }
        } else if self.cursor + 1 < self.matches.len() {
            self.cursor += 1;
        }
    }

    pub fn choose(&mut self) -> PickerOutcome {
        if let Some(account) = &self.account {
            return account
                .choices
                .get(account.cursor)
                .map_or(PickerOutcome::Pending, |choice| {
                    PickerOutcome::Selected(choice.target.clone())
                });
        }
        let Some(entry) = self.matches.get(self.cursor) else {
            return PickerOutcome::Pending;
        };
        match entry.accounts.as_slice() {
            [] => PickerOutcome::Pending,
            [single] => PickerOutcome::Selected(single.target.clone()),
            _ => {
                self.account = Some(AccountPicker {
                    choices: entry.accounts.clone(),
                    cursor: 0,
                });
                PickerOutcome::Pending
            }
        }
    }

    pub fn desired_height(&self) -> u16 {
        let rows = if let Some(account) = &self.account {
            account.choices.len()
        } else {
            self.matches.len().max(1)
        };
        u16::try_from(rows)
            .unwrap_or(u16::MAX)
            .min(12)
            .saturating_add(3)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        frame.render_widget(Clear, area);
        let block = Block::new()
            .borders(Borders::ALL)
            .border_style(theme.border())
            .style(theme.base());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let rows = usize::from(inner.height.saturating_sub(1));
        let mut lines: Vec<Line> = Vec::new();

        if let Some(account) = &self.account {
            lines.push(Line::from(Span::styled(" select account", theme.accent())));
            for (index, choice) in account.choices.iter().take(rows).enumerate() {
                let style = if index == account.cursor {
                    theme.selected()
                } else {
                    theme.base()
                };
                lines.push(Line::from(Span::styled(
                    format!("  {} ", choice.display),
                    style,
                )));
            }
            frame.render_widget(Paragraph::new(lines), inner);
            return;
        }

        let label = " select model   ";
        let filter = "filter: ";
        lines.push(Line::from(vec![
            Span::styled(label, theme.accent()),
            Span::styled(format!("{filter}{}", self.query), theme.muted()),
        ]));
        if self.matches.is_empty() {
            lines.push(Line::from(Span::styled("  no ready models", theme.muted())));
        } else {
            let start = if self.cursor >= rows {
                self.cursor + 1 - rows
            } else {
                0
            };
            let mut lines_remaining = rows;
            for (offset, entry) in self.matches.iter().skip(start).enumerate() {
                if lines_remaining == 0 {
                    break;
                }
                let global_idx = start + offset;
                let style = if global_idx == self.cursor {
                    theme.selected()
                } else {
                    theme.base()
                };
                lines.push(Line::from(Span::styled(
                    format!("  {}/{} ", entry.provider, entry.model),
                    style,
                )));
                lines_remaining -= 1;
            }
        }
        frame.render_widget(Paragraph::new(lines), inner);

        let col = label.chars().count() + filter.chars().count() + self.query.chars().count();
        let x = inner.x + u16::try_from(col).unwrap_or(u16::MAX);
        frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), inner.y));
    }
}

#[cfg(test)]
mod tests {
    use goat_protocol::{AccountChoice, ModelEntry, ModelTarget};

    use super::{Picker, PickerOutcome};

    fn entry(provider: &str, model: &str, accounts: usize) -> ModelEntry {
        let choices = (0..accounts)
            .map(|i| {
                let id = format!("acct-{i}");
                AccountChoice {
                    id: id.clone(),
                    display: id.clone(),
                    target: ModelTarget {
                        provider: provider.to_owned(),
                        model: model.to_owned(),
                        account: id,
                    },
                }
            })
            .collect();
        ModelEntry {
            provider: provider.to_owned(),
            model: model.to_owned(),
            accounts: choices,
        }
    }

    #[test]
    fn single_account_selects_directly() {
        let mut picker = Picker::new(vec![entry("openai", "gpt", 1)]);
        match picker.choose() {
            PickerOutcome::Selected(target) => {
                assert_eq!(target.provider, "openai");
                assert_eq!(target.model, "gpt");
            }
            PickerOutcome::Pending => panic!("expected direct selection"),
        }
    }

    #[test]
    fn multiple_accounts_open_interstitial() {
        let mut picker = Picker::new(vec![entry("openai", "gpt", 2)]);
        assert!(matches!(picker.choose(), PickerOutcome::Pending));
        picker.move_down();
        match picker.choose() {
            PickerOutcome::Selected(target) => assert_eq!(target.account, "acct-1"),
            PickerOutcome::Pending => panic!("expected account selection"),
        }
    }

    #[test]
    fn filter_narrows_matches() {
        let mut picker = Picker::new(vec![
            entry("openai", "gpt", 1),
            entry("anthropic", "claude", 1),
        ]);
        for ch in "claude".chars() {
            picker.on_char(ch);
        }
        match picker.choose() {
            PickerOutcome::Selected(target) => assert_eq!(target.provider, "anthropic"),
            PickerOutcome::Pending => panic!("expected filtered selection"),
        }
    }

    #[test]
    fn empty_choose_is_pending() {
        let mut picker = Picker::new(vec![]);
        assert!(matches!(picker.choose(), PickerOutcome::Pending));
    }
}
