use goat_protocol::{AccountChoice, Effort, ModelEntry, ModelTarget, ThreadSummary};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_normalization::UnicodeNormalization;
use unicode_width::UnicodeWidthStr;

use crate::{
    layout::{LIST_MAX, OVERLAY_CHROME, OVERLAY_CHROME_PLAIN, OVERLAY_W},
    overlay::{
        centered_rect, clamp_u16, hint_line, overlay_frame, overlay_layout, overlay_layout_plain,
        render_window, selection_row, truncate_to_width,
    },
    symbols,
    theme::Theme,
};

pub enum PickerOutcome {
    NoOp,
    Selected(ModelTarget),
}

fn scroll_input(input: &str, avail: usize) -> (String, usize) {
    let total = UnicodeWidthStr::width(input);
    if total < avail {
        return (input.to_owned(), 0);
    }
    let target = avail.saturating_sub(1).max(1);
    let mut acc = 0usize;
    let mut start_byte = input.len();
    for (i, ch) in input.char_indices().rev() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + w > target {
            break;
        }
        acc += w;
        start_byte = i;
    }
    let scrolled = total - acc;
    (input[start_byte..].to_owned(), scrolled)
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
        let lines: Vec<Line> = if self.matches.is_empty() {
            vec![Line::from(Span::styled(
                format!(
                    " no models yet {} run /config to connect a provider",
                    symbols::ui::ELLIPSIS
                ),
                theme.muted(),
            ))]
        } else {
            render_window(theme, width, self.cursor, self.matches.len(), rows, |idx| {
                let entry = &self.matches[idx];
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
                (vec![Span::styled(name, name_style)], right)
            })
        };
        frame.render_widget(Paragraph::new(lines), list_area);

        let _ = hint_area;

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
        }
    }

    fn cap(&self) -> usize {
        self.options.len().min(LIST_MAX)
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.options.len() {
            self.cursor += 1;
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
        let lines = render_window(theme, width, self.cursor, self.options.len(), rows, |idx| {
            let selected = idx == self.cursor;
            let name_style = if selected { theme.key() } else { theme.base() };
            (
                vec![Span::styled(
                    self.options[idx].as_str().to_owned(),
                    name_style,
                )],
                None,
            )
        });
        frame.render_widget(Paragraph::new(lines), list_area);

        let _ = hint_area;
    }
}

pub enum ThreadOutcome {
    NoOp,
    Selected(i64),
}

pub struct ThreadPicker {
    threads: Vec<ThreadSummary>,
    cursor: usize,
}

impl ThreadPicker {
    pub fn new(threads: Vec<ThreadSummary>) -> Self {
        Self { threads, cursor: 0 }
    }

    fn cap(&self) -> usize {
        self.threads.len().min(LIST_MAX)
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.threads.len() {
            self.cursor += 1;
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
        let lines: Vec<Line> = if self.threads.is_empty() {
            vec![Line::from(Span::styled(
                " no past conversations in this directory",
                theme.muted(),
            ))]
        } else {
            render_window(theme, width, self.cursor, self.threads.len(), rows, |idx| {
                let thread = &self.threads[idx];
                let selected = idx == self.cursor;
                let title_style = if selected { theme.key() } else { theme.base() };
                let left = vec![
                    Span::styled(format!("{}. ", idx + 1), theme.muted()),
                    Span::styled(thread.title.clone(), title_style),
                ];
                let right = Some(Span::styled(thread.model.clone(), theme.muted()));
                (left, right)
            })
        };
        frame.render_widget(Paragraph::new(lines), list_area);

        frame.render_widget(
            Paragraph::new(hint_line(&[(symbols::key::ENTER, "resume")], theme)),
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
    let lines = render_window(
        theme,
        width,
        account.cursor,
        account.choices.len(),
        rows,
        |idx| {
            let selected = idx == account.cursor;
            let name_style = if selected { theme.key() } else { theme.base() };
            (
                vec![Span::styled(
                    account.choices[idx].display.clone(),
                    name_style,
                )],
                None,
            )
        },
    );
    frame.render_widget(Paragraph::new(lines), list_area);

    frame.render_widget(
        Paragraph::new(hint_line(&[(symbols::key::ESC, "back")], theme)),
        hint_area,
    );
}

pub enum AskOutcome {
    NoOp,
    Pending,
    Submit(Vec<String>),
}

pub struct AskPicker {
    pub questions: Vec<goat_protocol::AskQuestion>,
    pub cursor: usize,
    current_q: usize,
    answers: Vec<Option<String>>,
    typing: bool,
    input: String,
    confirming: bool,
}

impl AskPicker {
    pub fn new(questions: Vec<goat_protocol::AskQuestion>) -> Self {
        let count = questions.len();
        Self {
            questions,
            cursor: 0,
            current_q: 0,
            answers: vec![None; count],
            typing: false,
            input: String::new(),
            confirming: false,
        }
    }

    pub fn is_confirming(&self) -> bool {
        self.confirming
    }

    pub fn is_typing(&self) -> bool {
        self.typing
    }

    pub fn insert_str(&mut self, text: &str) {
        let on_input_row = self.cursor == self.questions[self.current_q].options.len();
        if self.typing || on_input_row {
            self.typing = true;
            for ch in text.nfc() {
                self.input.push(ch);
            }
        }
    }

    pub fn move_up(&mut self) {
        if self.confirming {
            return;
        }
        if self.typing {
            self.typing = false;
        }
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.confirming {
            return;
        }
        if self.typing {
            self.typing = false;
        }
        let max = self.questions[self.current_q].options.len();
        if self.cursor < max {
            self.cursor += 1;
        }
    }

    pub fn on_char(&mut self, ch: char) {
        if self.confirming {
            return;
        }
        let on_input_row = self.cursor == self.questions[self.current_q].options.len();
        if self.typing || on_input_row {
            self.typing = true;
            self.input.push(ch);
        }
    }

    pub fn backspace(&mut self) {
        if self.typing {
            self.input.pop();
        }
    }

    pub fn choose(&mut self) -> AskOutcome {
        if self.confirming {
            return self.finish();
        }
        let type_own_idx = self.questions[self.current_q].options.len();
        if self.cursor == type_own_idx {
            if !self.input.is_empty() {
                let answer = std::mem::take(&mut self.input);
                self.typing = false;
                return self.record_answer(answer);
            }
            self.typing = true;
            return AskOutcome::Pending;
        }
        if let Some(opt) = self.questions[self.current_q].options.get(self.cursor) {
            let answer = opt.label.clone();
            self.cursor = 0;
            return self.record_answer(answer);
        }
        AskOutcome::NoOp
    }

    pub fn skip(&mut self) -> AskOutcome {
        if self.confirming {
            return AskOutcome::NoOp;
        }
        self.typing = false;
        self.input.clear();
        self.advance()
    }

    pub fn go_back(&mut self) {
        if self.confirming {
            self.confirming = false;
            let last = self.questions.len().saturating_sub(1);
            self.current_q = last;
            self.restore_cursor(last);
            return;
        }
        if self.typing {
            self.typing = false;
            return;
        }
        if self.current_q > 0 {
            self.current_q -= 1;
            self.restore_cursor(self.current_q);
        }
    }

    fn restore_cursor(&mut self, q_idx: usize) {
        self.typing = false;
        self.input.clear();
        self.cursor = 0;
        if let Some(Some(saved)) = self.answers.get(q_idx).cloned() {
            let pos = self.questions[q_idx]
                .options
                .iter()
                .position(|o| o.label == saved);
            if let Some(pos) = pos {
                self.cursor = pos;
            } else {
                self.input = saved;
                self.cursor = self.questions[q_idx].options.len();
            }
        }
    }

    fn record_answer(&mut self, answer: String) -> AskOutcome {
        self.answers[self.current_q] = Some(answer);
        self.advance()
    }

    fn advance(&mut self) -> AskOutcome {
        if self.current_q + 1 < self.questions.len() {
            self.current_q += 1;
            self.cursor = 0;
            self.typing = false;
            self.input.clear();
            AskOutcome::Pending
        } else if self.questions.len() > 1 {
            self.confirming = true;
            AskOutcome::Pending
        } else {
            self.finish()
        }
    }

    fn finish(&self) -> AskOutcome {
        let answers = self
            .answers
            .iter()
            .map(|a| a.clone().unwrap_or_default())
            .collect();
        AskOutcome::Submit(answers)
    }

    pub fn desired_height(&self) -> u16 {
        if self.confirming {
            let rows = clamp_u16(self.questions.len() * 2);
            rows.saturating_add(OVERLAY_CHROME)
        } else {
            let q = &self.questions[self.current_q];
            let rows = clamp_u16(q.options.len() + 1).min(clamp_u16(LIST_MAX));
            rows.saturating_add(OVERLAY_CHROME)
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let panel_h = self.desired_height();
        let [_, outer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(panel_h)]).areas(area);
        let Some(inner) = overlay_frame(frame, outer, theme) else {
            return;
        };
        if self.confirming {
            self.render_confirm(frame, inner, theme);
        } else {
            let (title_area, list_area, hint_area) = overlay_layout(inner);
            self.render_title(frame, title_area, theme);
            self.render_list(frame, list_area, theme);
            self.render_hint(frame, hint_area, theme);
        }
    }

    fn render_title(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let q = &self.questions[self.current_q];
        let total = self.questions.len();
        let dots = if total > 1 {
            let dot_str: Vec<&str> = (0..total)
                .map(|i| {
                    if i == self.current_q {
                        symbols::ui::DOT_FULL
                    } else {
                        symbols::ui::DOT_EMPTY
                    }
                })
                .collect();
            format!("  {}", dot_str.join(" "))
        } else {
            String::new()
        };
        let dots_w = UnicodeWidthStr::width(dots.as_str());
        let avail = usize::from(area.width).saturating_sub(dots_w + 1);
        let question = truncate_to_width(&q.question, avail);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {question}"), theme.base()),
                Span::styled(dots, theme.muted()),
            ])),
            area,
        );
    }

    fn render_list(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let q = &self.questions[self.current_q];
        let width = usize::from(area.width);
        let type_own_idx = q.options.len();
        let mut lines: Vec<Line> = Vec::new();
        for (i, opt) in q.options.iter().enumerate() {
            let selected = i == self.cursor;
            let label_style = theme.base();
            let right = opt
                .description
                .as_deref()
                .map(|d| Span::styled(d.to_owned(), theme.muted()));
            lines.push(selection_row(
                theme,
                selected,
                width,
                vec![Span::styled(opt.label.clone(), label_style)],
                right,
            ));
        }
        let input_selected = self.cursor == type_own_idx;
        let caret = if input_selected {
            Span::styled(format!(" {} ", symbols::ui::CARET), theme.accent())
        } else {
            Span::raw("   ")
        };
        let avail = width.saturating_sub(3).max(1);
        let (visible, scrolled) = scroll_input(&self.input, avail);
        let body = if self.typing || !self.input.is_empty() {
            Span::styled(visible, theme.base())
        } else {
            Span::styled("type your answer", theme.muted())
        };
        let row_style = if input_selected {
            theme.selected_row()
        } else {
            ratatui::style::Style::default()
        };
        let used = 3 + UnicodeWidthStr::width(body.content.as_ref());
        let pad = width.saturating_sub(used);
        lines.push(Line::from(vec![
            caret,
            Span::styled(
                body.content.clone().into_owned(),
                body.style.patch(row_style),
            ),
            Span::styled(" ".repeat(pad), row_style),
        ]));
        frame.render_widget(Paragraph::new(lines), area);
        if self.typing && input_selected {
            let row_y = area.y + clamp_u16(type_own_idx);
            let shown = UnicodeWidthStr::width(self.input.as_str()).saturating_sub(scrolled);
            let col = 3 + shown.min(avail);
            let x = area.x + clamp_u16(col);
            frame.set_cursor_position((x.min(area.right().saturating_sub(1)), row_y));
        }
    }

    fn render_confirm(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let (title_area, list_area, hint_area) = overlay_layout(area);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" confirm", theme.muted()))),
            title_area,
        );
        let mut lines: Vec<Line> = Vec::new();
        for (i, q) in self.questions.iter().enumerate() {
            let answer = self.answers[i].as_deref().filter(|s| !s.is_empty());
            let (answer_text, answer_style) = match answer {
                Some(a) => (a.to_owned(), theme.base()),
                None => ("—".to_owned(), theme.muted()),
            };
            lines.push(Line::from(Span::styled(
                format!("  {}", q.question),
                theme.muted(),
            )));
            lines.push(Line::from(Span::styled(
                format!("    {answer_text}"),
                answer_style,
            )));
        }
        frame.render_widget(Paragraph::new(lines), list_area);
        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::ENTER, "submit"),
                    (symbols::key::ARROW_LEFT, "edit"),
                ],
                theme,
            )),
            hint_area,
        );
    }

    fn render_hint(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let total = self.questions.len();
        let hint = if self.typing {
            hint_line(&[(symbols::key::ENTER, "confirm")], theme)
        } else if total > 1 {
            hint_line(
                &[
                    (symbols::key::ARROW_RIGHT, "skip"),
                    (symbols::key::ARROW_LEFT, "back"),
                ],
                theme,
            )
        } else {
            hint_line(&[], theme)
        };
        frame.render_widget(Paragraph::new(hint), area);
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
