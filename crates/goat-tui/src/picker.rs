use goat_protocol::{AccountChoice, Effort, ModelEntry, ModelTarget, ThreadSummary};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    layout::{LIST_MAX, OVERLAY_CHROME, OVERLAY_CHROME_PLAIN, OVERLAY_W},
    overlay::{
        centered_rect, clamp_u16, hint_line, overflow_hint, overlay_frame, overlay_layout,
        overlay_layout_plain, selection_row,
    },
    symbols,
    theme::Theme,
};

pub enum PickerOutcome {
    NoOp,
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
    current: Option<ModelTarget>,
    account: Option<AccountPicker>,
}

impl Picker {
    pub fn new(entries: Vec<ModelEntry>, current: Option<ModelTarget>) -> Self {
        let mut picker = Self {
            entries,
            query: String::new(),
            matches: Vec::new(),
            cursor: 0,
            current,
            account: None,
        };
        picker.refilter();
        picker.cursor = picker.current_index().unwrap_or(0);
        picker
    }

    fn current_index(&self) -> Option<usize> {
        let current = self.current.as_ref()?;
        self.matches
            .iter()
            .position(|e| e.provider == current.provider && e.model == current.model)
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
                .map_or(PickerOutcome::NoOp, |choice| {
                    PickerOutcome::Selected(choice.target.clone())
                });
        }
        let Some(entry) = self.matches.get(self.cursor) else {
            return PickerOutcome::NoOp;
        };
        match entry.accounts.as_slice() {
            [] => PickerOutcome::NoOp,
            [single] => PickerOutcome::Selected(single.target.clone()),
            _ => {
                self.account = Some(AccountPicker {
                    choices: entry.accounts.clone(),
                    cursor: 0,
                });
                PickerOutcome::NoOp
            }
        }
    }

    pub fn desired_height(&self) -> u16 {
        let rows = match &self.account {
            Some(account) => account.choices.len().max(1),
            None => self.matches.len().max(1),
        };
        clamp_u16(rows.min(LIST_MAX)).saturating_add(OVERLAY_CHROME)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let rect = centered_rect(area, OVERLAY_W, self.desired_height());
        let Some(inner) = overlay_frame(frame, rect, theme) else {
            return;
        };

        if let Some(account) = &self.account {
            render_account(frame, inner, theme, account);
            return;
        }

        let (input_area, list_area, hint_area) = overlay_layout(inner);

        let input_line = if self.query.is_empty() {
            Line::from(Span::styled(" search models", theme.muted()))
        } else {
            Line::from(Span::styled(format!(" {}", self.query), theme.base()))
        };
        frame.render_widget(Paragraph::new(input_line), input_area);

        let width = usize::from(list_area.width);
        let rows = usize::from(list_area.height);
        let mut lines: Vec<Line> = Vec::new();
        if self.matches.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(
                    " no models yet {} run /config to connect a provider",
                    symbols::ui::ELLIPSIS
                ),
                theme.muted(),
            )));
        } else {
            let start = if self.cursor >= rows {
                self.cursor + 1 - rows
            } else {
                0
            };
            let shown = rows.min(self.matches.len().saturating_sub(start));
            let (hint_above, hint_below) = overflow_hint(start, shown, self.matches.len());
            if let Some(ref above) = hint_above {
                lines.push(Line::from(Span::styled(format!(" {above}"), theme.muted())));
            }
            for (idx, entry) in self.matches.iter().enumerate().skip(start).take(rows) {
                let selected = idx == self.cursor;
                let is_current = self
                    .current
                    .as_ref()
                    .is_some_and(|c| c.model == entry.model && c.provider == entry.provider);
                let ctx = entry.context_window.map_or_else(String::new, |w| {
                    let k = w / 1000;
                    if k > 0 {
                        format!("{k}k")
                    } else {
                        format!("{w}")
                    }
                });
                let name = format!("{}/{}", entry.provider, entry.model);
                let name_style = if selected {
                    theme.key()
                } else if is_current {
                    theme.accent()
                } else {
                    theme.base()
                };
                let right = if ctx.is_empty() {
                    None
                } else {
                    Some(Span::styled(ctx, theme.muted()))
                };
                lines.push(selection_row(
                    theme,
                    selected,
                    width,
                    vec![Span::styled(name, name_style)],
                    right,
                ));
            }
            if let Some(ref below) = hint_below {
                lines.push(Line::from(Span::styled(format!(" {below}"), theme.muted())));
            }
        }
        frame.render_widget(Paragraph::new(lines), list_area);

        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::ARROWS_UPDOWN, "navigate"),
                    (symbols::key::ENTER, "select"),
                    (symbols::key::ESC, "close"),
                ],
                theme,
            )),
            hint_area,
        );

        let col = 1 + self.query.width();
        let x = input_area.x + clamp_u16(col);
        frame.set_cursor_position((x.min(input_area.right().saturating_sub(1)), input_area.y));
    }
}

pub enum EffortOutcome {
    NoOp,
    Selected(Effort),
}

pub struct EffortPicker {
    label: String,
    options: Vec<Effort>,
    cursor: usize,
    scroll: usize,
}

impl EffortPicker {
    pub fn new(label: String, options: Vec<Effort>, current: Option<Effort>) -> Self {
        let cursor = current
            .and_then(|cur| options.iter().position(|opt| *opt == cur))
            .unwrap_or(0);
        Self {
            label,
            options,
            cursor,
            scroll: 0,
        }
    }

    fn cap(&self) -> usize {
        self.options.len().min(LIST_MAX)
    }

    pub fn move_up(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 >= self.options.len() {
            return;
        }
        self.cursor += 1;
        let cap = self.cap();
        if self.cursor >= self.scroll + cap {
            self.scroll = self.cursor + 1 - cap;
        }
    }

    pub fn choose(&self) -> EffortOutcome {
        self.options
            .get(self.cursor)
            .map_or(EffortOutcome::NoOp, |effort| {
                EffortOutcome::Selected(*effort)
            })
    }

    pub fn desired_height(&self) -> u16 {
        clamp_u16(self.cap().max(1)).saturating_add(OVERLAY_CHROME)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let rect = centered_rect(area, OVERLAY_W, self.desired_height());
        let Some(inner) = overlay_frame(frame, rect, theme) else {
            return;
        };
        let (context_area, list_area, hint_area) = overlay_layout(inner);

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {}", self.label),
                theme.muted(),
            ))),
            context_area,
        );

        let width = usize::from(list_area.width);
        let rows = usize::from(list_area.height).max(1);
        let scroll = self.scroll.min(self.cursor);

        let lines: Vec<Line> = self
            .options
            .iter()
            .enumerate()
            .skip(scroll)
            .take(rows)
            .map(|(index, effort)| {
                let selected = index == self.cursor;
                let name_style = if selected { theme.key() } else { theme.base() };
                selection_row(
                    theme,
                    selected,
                    width,
                    vec![Span::styled(effort.as_str().to_owned(), name_style)],
                    None,
                )
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), list_area);

        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::ARROWS_UPDOWN, "navigate"),
                    (symbols::key::ENTER, "select"),
                    (symbols::key::ESC, "close"),
                ],
                theme,
            )),
            hint_area,
        );
    }
}

pub enum ThreadOutcome {
    NoOp,
    Selected(i64),
}

pub struct ThreadPicker {
    threads: Vec<ThreadSummary>,
    cursor: usize,
    scroll: usize,
}

impl ThreadPicker {
    pub fn new(threads: Vec<ThreadSummary>) -> Self {
        Self {
            threads,
            cursor: 0,
            scroll: 0,
        }
    }

    fn cap(&self) -> usize {
        self.threads.len().min(LIST_MAX)
    }

    fn visible_items(&self) -> usize {
        let cap = self.cap();
        if self.threads.len() > LIST_MAX {
            cap.saturating_sub(2)
        } else {
            cap
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 >= self.threads.len() {
            return;
        }
        self.cursor += 1;
        let vis = self.visible_items();
        if self.cursor >= self.scroll + vis {
            self.scroll = self.cursor + 1 - vis;
        }
    }

    pub fn choose(&self) -> ThreadOutcome {
        self.threads
            .get(self.cursor)
            .map_or(ThreadOutcome::NoOp, |thread| {
                ThreadOutcome::Selected(thread.id)
            })
    }

    pub fn desired_height(&self) -> u16 {
        clamp_u16(self.cap().max(1)).saturating_add(OVERLAY_CHROME_PLAIN)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let rect = centered_rect(area, OVERLAY_W, self.desired_height());
        let Some(inner) = overlay_frame(frame, rect, theme) else {
            return;
        };
        let (list_area, hint_area) = overlay_layout_plain(inner);

        let width = usize::from(list_area.width);
        let rows = usize::from(list_area.height).max(1);
        let scroll = self.scroll.min(self.cursor);
        let mut lines: Vec<Line> = Vec::new();
        if self.threads.is_empty() {
            lines.push(Line::from(Span::styled(
                " no past conversations in this directory",
                theme.muted(),
            )));
        } else {
            let above_rows = usize::from(scroll > 0);
            let budget = rows.saturating_sub(above_rows);
            let remaining = self.threads.len().saturating_sub(scroll);
            let has_below = remaining > budget;
            let take = if has_below {
                budget.saturating_sub(1)
            } else {
                budget.min(remaining)
            };

            if scroll > 0 {
                lines.push(Line::from(Span::styled(
                    format!(" {} {} more", symbols::ui::MORE_ABOVE, scroll),
                    theme.muted(),
                )));
            }
            for (idx, thread) in self.threads.iter().enumerate().skip(scroll).take(take) {
                let selected = idx == self.cursor;
                let title_style = if selected { theme.key() } else { theme.base() };
                let left = vec![
                    Span::styled(format!("{}. ", idx + 1), theme.muted()),
                    Span::styled(thread.title.clone(), title_style),
                ];
                let right = Some(Span::styled(thread.model.clone(), theme.muted()));
                lines.push(selection_row(theme, selected, width, left, right));
            }
            if has_below {
                let hidden = self.threads.len() - scroll - take;
                lines.push(Line::from(Span::styled(
                    format!(" {} {} more", symbols::ui::MORE_BELOW, hidden),
                    theme.muted(),
                )));
            }
        }
        frame.render_widget(Paragraph::new(lines), list_area);

        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::ARROWS_UPDOWN, "navigate"),
                    (symbols::key::ENTER, "resume"),
                    (symbols::key::ESC, "close"),
                ],
                theme,
            )),
            hint_area,
        );
    }
}

fn render_account(frame: &mut Frame, inner: Rect, theme: Theme, account: &AccountPicker) {
    let (context_area, list_area, hint_area) = overlay_layout(inner);

    let model_label = account.choices.first().map_or_else(String::new, |c| {
        format!("{}/{}", c.target.provider, c.target.model)
    });
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {model_label}"),
            theme.muted(),
        ))),
        context_area,
    );

    let width = usize::from(list_area.width);
    let rows = usize::from(list_area.height);
    let lines: Vec<Line> = account
        .choices
        .iter()
        .take(rows)
        .enumerate()
        .map(|(index, choice)| {
            let selected = index == account.cursor;
            let name_style = if selected { theme.key() } else { theme.base() };
            selection_row(
                theme,
                selected,
                width,
                vec![Span::styled(choice.display.clone(), name_style)],
                None,
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list_area);

    frame.render_widget(
        Paragraph::new(hint_line(
            &[
                (symbols::key::ARROWS_UPDOWN, "navigate"),
                (symbols::key::ENTER, "select"),
                (symbols::key::ESC, "back"),
            ],
            theme,
        )),
        hint_area,
    );
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
                        effort: None,
                    },
                }
            })
            .collect();
        ModelEntry {
            provider: provider.to_owned(),
            model: model.to_owned(),
            accounts: choices,
            context_window: None,
            efforts: Vec::new(),
        }
    }

    #[test]
    fn single_account_selects_directly() {
        let mut picker = Picker::new(vec![entry("openai", "gpt", 1)], None);
        match picker.choose() {
            PickerOutcome::Selected(target) => {
                assert_eq!(target.provider, "openai");
                assert_eq!(target.model, "gpt");
            }
            PickerOutcome::NoOp => panic!("expected direct selection"),
        }
    }

    #[test]
    fn multiple_accounts_open_interstitial() {
        let mut picker = Picker::new(vec![entry("openai", "gpt", 2)], None);
        assert!(matches!(picker.choose(), PickerOutcome::NoOp));
        picker.move_down();
        match picker.choose() {
            PickerOutcome::Selected(target) => assert_eq!(target.account, "acct-1"),
            PickerOutcome::NoOp => panic!("expected account selection"),
        }
    }

    #[test]
    fn filter_narrows_matches() {
        let mut picker = Picker::new(
            vec![entry("openai", "gpt", 1), entry("anthropic", "claude", 1)],
            None,
        );
        for ch in "claude".chars() {
            picker.on_char(ch);
        }
        match picker.choose() {
            PickerOutcome::Selected(target) => assert_eq!(target.provider, "anthropic"),
            PickerOutcome::NoOp => panic!("expected filtered selection"),
        }
    }

    #[test]
    fn empty_choose_is_noop() {
        let mut picker = Picker::new(vec![], None);
        assert!(matches!(picker.choose(), PickerOutcome::NoOp));
    }
}
