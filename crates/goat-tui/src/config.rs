use goat_protocol::{AccountEntry, AuthMethod, LoginCredential};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    layout::{OVERLAY_CHROME, OVERLAY_W},
    overlay::{centered_rect, clamp_u16, hint_line, overlay_frame, overlay_layout, selection_row},
    symbols,
    theme::Theme,
};

const FIELD_LABEL_W: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Section {
    Providers,
    Appearance,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Field {
    Name,
    Key,
}

enum InputStage {
    List,
    Choosing {
        provider: String,
        method: AuthMethod,
    },
    Adding {
        provider: String,
        method: AuthMethod,
        name: String,
        key: String,
        field: Field,
    },
    Waiting {
        provider: String,
        method: AuthMethod,
        name: String,
        status: Option<String>,
    },
}

pub enum StageKind {
    List,
    Input,
    Waiting,
}

pub enum ConfigOutcome {
    Pending,
    AddAccount {
        provider: String,
        name: String,
        credential: LoginCredential,
    },
    RemoveAccount {
        provider: String,
        name: String,
    },
    SetTheme {
        dark: bool,
    },
    SetMouseCapture {
        enabled: bool,
    },
    SetComputerUse {
        enabled: bool,
    },
    SetBrowser {
        enabled: bool,
    },
}

#[derive(Clone)]
struct Row {
    kind: RowKind,
    provider_index: usize,
    account_index: Option<usize>,
}

#[derive(Clone, PartialEq)]
enum RowKind {
    ProviderHeader,
    Account,
    AddAccount,
}

pub struct Config {
    section: Section,
    providers: Vec<AccountEntry>,
    cursor: usize,
    stage: InputStage,
    dark_theme: bool,
    mouse_capture: bool,
    computer_use: bool,
    browser: bool,
    error: Option<String>,
}

impl Config {
    pub fn new(
        providers: Vec<AccountEntry>,
        dark_theme: bool,
        mouse_capture: bool,
        computer_use: bool,
        browser: bool,
    ) -> Self {
        let mut config = Self {
            section: Section::Providers,
            providers,
            cursor: 0,
            stage: InputStage::List,
            dark_theme,
            mouse_capture,
            computer_use,
            browser,
            error: None,
        };
        config.cursor = config.first_selectable();
        config
    }

    pub fn set_providers(&mut self, providers: Vec<AccountEntry>) {
        self.providers = providers;
        if matches!(self.section, Section::Providers) && !self.is_selectable(self.cursor) {
            self.cursor = self.first_selectable();
        }
    }

    pub fn set_account_status(&mut self, message: String) {
        if let InputStage::Waiting { status, .. } = &mut self.stage {
            *status = Some(message);
        }
    }

    pub fn set_error(&mut self, message: String) {
        self.error = Some(message);
        if let InputStage::Waiting {
            provider,
            method,
            name,
            ..
        } = &self.stage
        {
            let field = if matches!(method, AuthMethod::OAuth) {
                Field::Name
            } else {
                Field::Key
            };
            self.stage = InputStage::Adding {
                provider: provider.clone(),
                method: *method,
                name: name.clone(),
                key: String::new(),
                field,
            };
        }
    }

    fn provider_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (pi, entry) in self.providers.iter().enumerate() {
            rows.push(Row {
                kind: RowKind::ProviderHeader,
                provider_index: pi,
                account_index: None,
            });
            if entry.local {
                continue;
            }
            for (ai, _) in entry.accounts.iter().enumerate() {
                rows.push(Row {
                    kind: RowKind::Account,
                    provider_index: pi,
                    account_index: Some(ai),
                });
            }
            rows.push(Row {
                kind: RowKind::AddAccount,
                provider_index: pi,
                account_index: None,
            });
        }
        rows
    }

    fn is_selectable(&self, index: usize) -> bool {
        match self.section {
            Section::Providers => self
                .provider_rows()
                .get(index)
                .is_some_and(|row| row.kind != RowKind::ProviderHeader),
            Section::Appearance => index <= 3,
        }
    }

    fn first_selectable(&self) -> usize {
        match self.section {
            Section::Providers => self
                .provider_rows()
                .iter()
                .position(|row| row.kind != RowKind::ProviderHeader)
                .unwrap_or(0),
            Section::Appearance => 0,
        }
    }

    pub fn move_up(&mut self) {
        self.error = None;
        if let InputStage::Choosing { method, .. } = &mut self.stage {
            *method = AuthMethod::ApiKey;
            return;
        }
        if !matches!(self.stage, InputStage::List) {
            return;
        }
        match self.section {
            Section::Providers => {
                let mut index = self.cursor;
                while index > 0 {
                    index -= 1;
                    if self.is_selectable(index) {
                        self.cursor = index;
                        return;
                    }
                }
            }
            Section::Appearance => self.cursor = self.cursor.saturating_sub(1),
        }
    }

    pub fn move_down(&mut self) {
        self.error = None;
        if let InputStage::Choosing { method, .. } = &mut self.stage {
            *method = AuthMethod::OAuth;
            return;
        }
        if !matches!(self.stage, InputStage::List) {
            return;
        }
        match self.section {
            Section::Providers => {
                let last = self.provider_rows().len().saturating_sub(1);
                let mut index = self.cursor;
                while index < last {
                    index += 1;
                    if self.is_selectable(index) {
                        self.cursor = index;
                        return;
                    }
                }
            }
            Section::Appearance => {
                if self.cursor < 3 {
                    self.cursor += 1;
                }
            }
        }
    }

    pub fn on_char(&mut self, ch: char) {
        if let InputStage::Adding {
            name, key, field, ..
        } = &mut self.stage
        {
            match field {
                Field::Name => name.push(ch),
                Field::Key => key.push(ch),
            }
        }
    }

    pub fn backspace(&mut self) {
        if let InputStage::Adding {
            name, key, field, ..
        } = &mut self.stage
        {
            match field {
                Field::Name => {
                    name.pop();
                }
                Field::Key => {
                    key.pop();
                }
            }
        }
    }

    pub fn tab(&mut self) {
        self.error = None;
        match &mut self.stage {
            InputStage::List => {
                self.section = match self.section {
                    Section::Providers => Section::Appearance,
                    Section::Appearance => Section::Providers,
                };
                self.cursor = self.first_selectable();
            }
            InputStage::Adding { method, field, .. } => {
                if matches!(method, AuthMethod::ApiKey) {
                    *field = match field {
                        Field::Name => Field::Key,
                        Field::Key => Field::Name,
                    };
                }
            }
            InputStage::Choosing { .. } | InputStage::Waiting { .. } => {}
        }
    }

    pub fn enter(&mut self) -> ConfigOutcome {
        self.error = None;
        match &self.stage {
            InputStage::List => self.enter_list(),
            InputStage::Choosing { .. } => self.enter_choosing(),
            InputStage::Adding { .. } => self.enter_adding(),
            InputStage::Waiting { .. } => ConfigOutcome::Pending,
        }
    }

    fn enter_list(&mut self) -> ConfigOutcome {
        match self.section {
            Section::Providers => {
                let rows = self.provider_rows();
                let Some(row) = rows.get(self.cursor) else {
                    return ConfigOutcome::Pending;
                };
                if row.kind == RowKind::AddAccount {
                    let entry = &self.providers[row.provider_index];
                    let provider = entry.provider.clone();
                    let method = provider_method(entry);
                    self.stage = if matches!(method, AuthMethod::ApiKeyOrOAuth) {
                        InputStage::Choosing {
                            provider,
                            method: AuthMethod::ApiKey,
                        }
                    } else {
                        InputStage::Adding {
                            provider,
                            method,
                            name: "default".to_owned(),
                            key: String::new(),
                            field: Field::Name,
                        }
                    };
                }
                ConfigOutcome::Pending
            }
            Section::Appearance => match self.cursor {
                0 => {
                    let dark = !self.dark_theme;
                    self.dark_theme = dark;
                    ConfigOutcome::SetTheme { dark }
                }
                1 => {
                    let enabled = !self.mouse_capture;
                    self.mouse_capture = enabled;
                    ConfigOutcome::SetMouseCapture { enabled }
                }
                2 => {
                    let enabled = !self.computer_use;
                    self.computer_use = enabled;
                    ConfigOutcome::SetComputerUse { enabled }
                }
                3 => {
                    let enabled = !self.browser;
                    self.browser = enabled;
                    ConfigOutcome::SetBrowser { enabled }
                }
                _ => ConfigOutcome::Pending,
            },
        }
    }

    fn enter_adding(&mut self) -> ConfigOutcome {
        let InputStage::Adding {
            provider,
            method,
            name,
            key,
            field,
        } = &mut self.stage
        else {
            return ConfigOutcome::Pending;
        };
        if name.trim().is_empty() {
            *field = Field::Name;
            return ConfigOutcome::Pending;
        }
        let method = *method;
        if matches!(method, AuthMethod::OAuth) {
            let provider = provider.clone();
            let name = name.clone();
            self.stage = InputStage::Waiting {
                provider: provider.clone(),
                method,
                name: name.clone(),
                status: Some(format!("opening browser{}", symbols::ui::ELLIPSIS)),
            };
            ConfigOutcome::AddAccount {
                provider,
                name,
                credential: LoginCredential::OAuth,
            }
        } else {
            if key.is_empty() {
                *field = Field::Key;
                return ConfigOutcome::Pending;
            }
            let provider = provider.clone();
            let name = name.clone();
            let key = key.clone();
            self.stage = InputStage::Waiting {
                provider: provider.clone(),
                method,
                name: name.clone(),
                status: Some(format!("validating{}", symbols::ui::ELLIPSIS)),
            };
            ConfigOutcome::AddAccount {
                provider,
                name,
                credential: LoginCredential::ApiKey(key),
            }
        }
    }

    fn enter_choosing(&mut self) -> ConfigOutcome {
        let InputStage::Choosing { provider, method } = &self.stage else {
            return ConfigOutcome::Pending;
        };
        self.stage = InputStage::Adding {
            provider: provider.clone(),
            method: *method,
            name: "default".to_owned(),
            key: String::new(),
            field: Field::Name,
        };
        ConfigOutcome::Pending
    }

    pub fn remove_selected(&mut self) -> ConfigOutcome {
        self.error = None;
        if !matches!(self.section, Section::Providers) {
            return ConfigOutcome::Pending;
        }
        if !matches!(self.stage, InputStage::List) {
            return ConfigOutcome::Pending;
        }
        let rows = self.provider_rows();
        let Some(row) = rows.get(self.cursor) else {
            return ConfigOutcome::Pending;
        };
        if row.kind != RowKind::Account {
            return ConfigOutcome::Pending;
        }
        let entry = &self.providers[row.provider_index];
        let Some(ai) = row.account_index else {
            return ConfigOutcome::Pending;
        };
        let provider = entry.provider.clone();
        let name = entry.accounts[ai].name.clone();
        ConfigOutcome::RemoveAccount { provider, name }
    }

    pub fn cancel_stage(&mut self) {
        self.error = None;
        self.stage = InputStage::List;
    }

    pub fn stage_kind(&self) -> StageKind {
        match self.stage {
            InputStage::List => StageKind::List,
            InputStage::Choosing { .. } | InputStage::Adding { .. } => StageKind::Input,
            InputStage::Waiting { .. } => StageKind::Waiting,
        }
    }

    pub fn desired_height(&self) -> u16 {
        match self.stage {
            InputStage::List => {
                let rows = match self.section {
                    Section::Providers => {
                        self.provider_rows().len() + self.providers.len().saturating_sub(1)
                    }
                    Section::Appearance => 4,
                };
                clamp_u16(rows.max(1))
                    .saturating_add(OVERLAY_CHROME)
                    .clamp(10, 30)
            }
            InputStage::Choosing { .. } => 8,
            InputStage::Adding { .. } => 9,
            InputStage::Waiting { .. } => 6,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let rect = centered_rect(area, OVERLAY_W, self.desired_height());
        let Some(inner) = overlay_frame(frame, rect, theme) else {
            return;
        };
        match &self.stage {
            InputStage::List => {
                let (tabs_area, body_area, hint_area) = overlay_layout(inner);
                self.render_tabs(frame, tabs_area, theme);
                self.render_list(frame, body_area, hint_area, theme);
            }
            InputStage::Adding {
                provider,
                method,
                name,
                key,
                field,
            } => {
                render_adding(
                    frame,
                    inner,
                    theme,
                    provider,
                    *method,
                    name,
                    key,
                    *field,
                    self.error.as_deref(),
                );
            }
            InputStage::Waiting {
                provider,
                name,
                status,
                ..
            } => {
                render_waiting(frame, inner, theme, provider, name, status.as_deref());
            }
            InputStage::Choosing { provider, method } => {
                render_choosing(frame, inner, theme, provider, *method);
            }
        }
    }

    fn render_tabs(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let providers_style = if self.section == Section::Providers {
            theme.accent()
        } else {
            theme.muted()
        };
        let appearance_style = if self.section == Section::Appearance {
            theme.accent()
        } else {
            theme.muted()
        };
        let line = Line::from(vec![
            Span::raw(" "),
            Span::styled("providers", providers_style),
            Span::raw("   "),
            Span::styled("appearance", appearance_style),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn render_list(&self, frame: &mut Frame, body_area: Rect, hint_area: Rect, theme: Theme) {
        match self.section {
            Section::Providers => {
                self.render_providers(frame, body_area, theme);
                frame.render_widget(
                    Paragraph::new(hint_line(
                        &[
                            (symbols::key::ARROWS_UPDOWN, "move"),
                            (symbols::key::ENTER, "add"),
                            (symbols::key::BACKSPACE, "remove"),
                            (symbols::key::ARROWS_LEFTRIGHT, "section"),
                            (symbols::key::ESC, "close"),
                        ],
                        theme,
                    )),
                    hint_area,
                );
            }
            Section::Appearance => {
                self.render_appearance(frame, body_area, theme);
                frame.render_widget(
                    Paragraph::new(hint_line(
                        &[
                            (symbols::key::ARROWS_UPDOWN, "move"),
                            (symbols::key::ENTER, "toggle"),
                            (symbols::key::ARROWS_LEFTRIGHT, "section"),
                            (symbols::key::ESC, "close"),
                        ],
                        theme,
                    )),
                    hint_area,
                );
            }
        }
    }

    fn render_providers(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let rows = self.provider_rows();
        let mut lines: Vec<Line> = Vec::new();
        let mut cursor_line = 0usize;
        let mut seen_header = false;
        for (i, row) in rows.iter().enumerate() {
            let selected = i == self.cursor;
            let entry = &self.providers[row.provider_index];
            match row.kind {
                RowKind::ProviderHeader => {
                    if seen_header {
                        lines.push(Line::default());
                    }
                    seen_header = true;
                    let mut spans =
                        vec![Span::styled(format!(" {}", entry.provider), theme.accent())];
                    if entry.local {
                        spans.push(Span::styled(
                            format!("{}local", symbols::ui::SEPARATOR),
                            theme.muted(),
                        ));
                    }
                    lines.push(Line::from(spans));
                }
                RowKind::Account => {
                    let ai = row.account_index.unwrap_or(0);
                    let account = &entry.accounts[ai];
                    let name_style = if selected { theme.key() } else { theme.base() };
                    let left = vec![Span::styled(account.name.clone(), name_style)];
                    let right = Some(Span::styled(method_label(account.method), theme.muted()));
                    if selected {
                        cursor_line = lines.len();
                    }
                    lines.push(selection_row(
                        theme,
                        selected,
                        usize::from(area.width),
                        left,
                        right,
                    ));
                }
                RowKind::AddAccount => {
                    let style = if selected { theme.key() } else { theme.muted() };
                    if selected {
                        cursor_line = lines.len();
                    }
                    lines.push(selection_row(
                        theme,
                        selected,
                        usize::from(area.width),
                        vec![Span::styled("+ add account", style)],
                        None,
                    ));
                }
            }
        }

        let height = usize::from(area.height).max(1);
        let total = lines.len();
        if total <= height {
            frame.render_widget(Paragraph::new(lines), area);
            return;
        }
        let cap = height.saturating_sub(2).max(1);
        let start = cursor_line
            .saturating_sub(cap.saturating_sub(1))
            .min(total - cap);
        let mut out: Vec<Line> = Vec::new();
        if start > 0 {
            out.push(Line::from(Span::styled(
                format!(" {} {} more", symbols::ui::MORE_ABOVE, start),
                theme.muted(),
            )));
        }
        out.extend(lines.into_iter().skip(start).take(cap));
        let below = total - start - cap.min(total - start);
        if below > 0 {
            out.push(Line::from(Span::styled(
                format!(" {} {} more", symbols::ui::MORE_BELOW, below),
                theme.muted(),
            )));
        }
        frame.render_widget(Paragraph::new(out), area);
    }

    fn render_appearance(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let width = usize::from(area.width);
        let lines = vec![
            appearance_row(
                theme,
                width,
                self.cursor == 0,
                "theme",
                self.dark_theme,
                "dark",
                "light",
            ),
            appearance_row(
                theme,
                width,
                self.cursor == 1,
                "mouse",
                self.mouse_capture,
                "on",
                "off",
            ),
            appearance_row(
                theme,
                width,
                self.cursor == 2,
                "computer use",
                self.computer_use,
                "on",
                "off",
            ),
            appearance_row(
                theme,
                width,
                self.cursor == 3,
                "browser",
                self.browser,
                "on",
                "off",
            ),
        ];
        frame.render_widget(Paragraph::new(lines), area);
    }
}

fn appearance_row(
    theme: Theme,
    width: usize,
    selected: bool,
    label: &str,
    first_active: bool,
    first: &str,
    second: &str,
) -> Line<'static> {
    let label_style = if selected { theme.key() } else { theme.base() };
    let first_dot = if first_active {
        symbols::ui::DOT_FULL
    } else {
        symbols::ui::DOT_EMPTY
    };
    let second_dot = if first_active {
        symbols::ui::DOT_EMPTY
    } else {
        symbols::ui::DOT_FULL
    };
    let first_style = if first_active {
        theme.accent()
    } else {
        theme.muted()
    };
    let second_style = if first_active {
        theme.muted()
    } else {
        theme.accent()
    };
    selection_row(
        theme,
        selected,
        width,
        vec![
            Span::styled(format!("{label:<12}"), label_style),
            Span::styled(format!("{first_dot} {first:<6}"), first_style),
            Span::styled(format!("{second_dot} {second}"), second_style),
        ],
        None,
    )
}

fn method_label(method: AuthMethod) -> &'static str {
    match method {
        AuthMethod::ApiKey => "api key",
        AuthMethod::OAuth => "browser",
        AuthMethod::ApiKeyOrOAuth => "api key / browser",
        AuthMethod::None => "no auth",
    }
}

fn provider_method(entry: &AccountEntry) -> AuthMethod {
    entry.login
}

#[allow(clippy::too_many_arguments)]
fn render_adding(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    provider: &str,
    method: AuthMethod,
    name: &str,
    key: &str,
    field: Field,
    error: Option<&str>,
) {
    let [title_area, _, name_area, key_area, _, error_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    let title = Line::from(vec![
        Span::styled(format!(" {provider}"), theme.key()),
        Span::styled(
            format!("{}new account", symbols::ui::SEPARATOR),
            theme.muted(),
        ),
    ]);
    frame.render_widget(Paragraph::new(title), title_area);

    let api_key = !matches!(method, AuthMethod::OAuth);
    let value_cols = usize::from(area.width).saturating_sub(3 + FIELD_LABEL_W + 1);
    let name_active = field == Field::Name;
    let name_label_style = if name_active {
        theme.accent()
    } else {
        theme.muted()
    };
    let shown_name = input_tail(name, value_cols);
    let name_cols = shown_name.width();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("   {:<FIELD_LABEL_W$}", "name"), name_label_style),
            Span::styled(shown_name, theme.base()),
        ])),
        name_area,
    );
    if name_active {
        place_cursor(frame, name_area, name_cols);
    }

    if api_key {
        let key_active = field == Field::Key;
        let key_label_style = if key_active {
            theme.accent()
        } else {
            theme.muted()
        };
        let mask_cols = key.chars().count().min(value_cols);
        let masked = symbols::ui::MASK.repeat(mask_cols);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("   {:<FIELD_LABEL_W$}", "key"), key_label_style),
                Span::styled(masked, theme.base()),
            ])),
            key_area,
        );
        if key_active {
            place_cursor(frame, key_area, mask_cols);
        }
    }

    if let Some(message) = error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("   {} {message}", symbols::ui::CROSS),
                theme.error(),
            )))
            .wrap(Wrap { trim: false }),
            error_area,
        );
    }

    if api_key {
        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::ENTER, "save"),
                    (symbols::key::TAB, "next field"),
                    (symbols::key::ESC, "cancel"),
                ],
                theme,
            )),
            hint_area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(hint_line(
                &[
                    (symbols::key::ENTER, "open browser"),
                    (symbols::key::ESC, "cancel"),
                ],
                theme,
            )),
            hint_area,
        );
    }
}

fn input_tail(value: &str, max_cols: usize) -> String {
    if value.width() <= max_cols {
        return value.to_owned();
    }
    let mut cols = 0usize;
    let mut tail: Vec<char> = Vec::new();
    for c in value.chars().rev() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if cols + w > max_cols {
            break;
        }
        cols += w;
        tail.push(c);
    }
    tail.iter().rev().collect()
}

fn place_cursor(frame: &mut Frame, area: Rect, value_cols: usize) {
    let prefix = clamp_u16(3 + FIELD_LABEL_W);
    let col = clamp_u16(value_cols).saturating_add(prefix);
    let x = area.x + col.min(area.width.saturating_sub(1));
    frame.set_cursor_position((x, area.y));
}

fn render_waiting(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    provider: &str,
    name: &str,
    status: Option<&str>,
) {
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!(" {provider}"), theme.key()),
            Span::styled(symbols::ui::SEPARATOR, theme.muted()),
            Span::styled(name.to_owned(), theme.base()),
        ]),
        Line::from(Span::raw("")),
    ];
    if let Some(msg) = status {
        lines.push(Line::from(Span::styled(format!("   {msg}"), theme.muted())));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_choosing(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    provider: &str,
    method: AuthMethod,
) {
    let [title_area, _, api_area, browser_area, _, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    let title = Line::from(vec![
        Span::styled(format!(" {provider}"), theme.key()),
        Span::styled(
            format!("{}sign in with", symbols::ui::SEPARATOR),
            theme.muted(),
        ),
    ]);
    frame.render_widget(Paragraph::new(title), title_area);

    let width = usize::from(area.width);
    let row = |selected: bool, label: &str| {
        let style = if selected { theme.key() } else { theme.base() };
        selection_row(
            theme,
            selected,
            width,
            vec![Span::styled(label.to_owned(), style)],
            None,
        )
    };
    frame.render_widget(
        Paragraph::new(row(matches!(method, AuthMethod::ApiKey), "api key")),
        api_area,
    );
    frame.render_widget(
        Paragraph::new(row(matches!(method, AuthMethod::OAuth), "browser")),
        browser_area,
    );
    frame.render_widget(
        Paragraph::new(hint_line(
            &[
                (symbols::key::ARROWS_UPDOWN, "choose"),
                (symbols::key::ENTER, "continue"),
                (symbols::key::ESC, "cancel"),
            ],
            theme,
        )),
        hint_area,
    );
}

#[cfg(test)]
mod tests {
    use goat_protocol::{AccountEntry, AccountInfo, AuthMethod, LoginCredential};

    use super::{Config, ConfigOutcome, Field};

    fn make_providers() -> Vec<AccountEntry> {
        vec![
            AccountEntry {
                provider: "anthropic".to_owned(),
                display_name: "anthropic".to_owned(),
                accounts: vec![AccountInfo {
                    name: "work-key".to_owned(),
                    method: AuthMethod::ApiKey,
                }],
                local: false,
                login: AuthMethod::ApiKey,
            },
            AccountEntry {
                provider: "ollama".to_owned(),
                display_name: "ollama".to_owned(),
                accounts: Vec::new(),
                local: true,
                login: AuthMethod::None,
            },
        ]
    }

    fn oauth_provider() -> Vec<AccountEntry> {
        vec![AccountEntry {
            provider: "anthropic".to_owned(),
            display_name: "anthropic".to_owned(),
            accounts: Vec::new(),
            local: false,
            login: AuthMethod::ApiKeyOrOAuth,
        }]
    }

    #[test]
    fn oauth_choice_then_browser_flow() {
        let mut config = Config::new(oauth_provider(), true, true, false, false);
        config.enter();
        assert!(matches!(config.stage, super::InputStage::Choosing { .. }));
        config.move_down();
        config.enter();
        assert!(matches!(
            config.stage,
            super::InputStage::Adding {
                method: AuthMethod::OAuth,
                ..
            }
        ));
        let out = config.enter();
        assert!(matches!(
            out,
            ConfigOutcome::AddAccount {
                ref provider,
                ref credential,
                ..
            } if provider == "anthropic" && matches!(credential, LoginCredential::OAuth)
        ));
        assert!(matches!(config.stage, super::InputStage::Waiting { .. }));
    }

    #[test]
    fn oauth_choice_api_key_branch() {
        let mut config = Config::new(oauth_provider(), true, true, false, false);
        config.enter();
        config.enter();
        assert!(matches!(
            config.stage,
            super::InputStage::Adding {
                method: AuthMethod::ApiKey,
                ..
            }
        ));
    }

    #[test]
    fn tab_switches_section() {
        let mut config = Config::new(make_providers(), true, true, false, false);
        assert_eq!(config.section, super::Section::Providers);
        config.tab();
        assert_eq!(config.section, super::Section::Appearance);
        config.tab();
        assert_eq!(config.section, super::Section::Providers);
    }

    #[test]
    fn move_down_skips_provider_headers() {
        let config_rows = Config::new(make_providers(), true, true, false, false);
        assert_eq!(config_rows.cursor, 1);
        let mut config = config_rows;
        config.move_down();
        assert_eq!(config.cursor, 2);
        config.move_down();
        assert_eq!(config.cursor, 2);
    }

    #[test]
    fn add_account_flow_api_key() {
        let mut config = Config::new(make_providers(), true, true, false, false);
        config.move_down();
        let out = config.enter();
        assert!(matches!(out, ConfigOutcome::Pending));
        for _ in 0.."default".len() {
            config.backspace();
        }
        for ch in "mykey".chars() {
            config.on_char(ch);
        }
        let out2 = config.enter();
        assert!(matches!(out2, ConfigOutcome::Pending));
        if let super::InputStage::Adding { field, .. } = &config.stage {
            assert_eq!(*field, Field::Key);
        } else {
            panic!("expected Adding stage");
        }
        for ch in "sk-test".chars() {
            config.on_char(ch);
        }
        let out3 = config.enter();
        assert!(matches!(
            out3,
            ConfigOutcome::AddAccount { ref provider, ref name, ref credential }
            if provider == "anthropic" && name == "mykey" && matches!(credential, LoginCredential::ApiKey(k) if k == "sk-test")
        ));
    }

    #[test]
    fn remove_account_row() {
        let mut config = Config::new(make_providers(), true, true, false, false);
        let out = config.remove_selected();
        assert!(matches!(
            out,
            ConfigOutcome::RemoveAccount { ref provider, ref name }
            if provider == "anthropic" && name == "work-key"
        ));
    }

    #[test]
    fn theme_toggle_in_appearance() {
        let mut config = Config::new(make_providers(), true, true, false, false);
        config.tab();
        let out = config.enter();
        assert!(matches!(out, ConfigOutcome::SetTheme { dark: false }));
        assert!(!config.dark_theme);
    }

    #[test]
    fn backspace_clears_input() {
        let mut config = Config::new(make_providers(), true, true, false, false);
        config.move_down();
        config.enter();
        for _ in 0.."default".len() {
            config.backspace();
        }
        config.on_char('a');
        config.on_char('b');
        config.backspace();
        if let super::InputStage::Adding { name, .. } = &config.stage {
            assert_eq!(name, "a");
        } else {
            panic!("expected Adding stage");
        }
    }

    #[test]
    fn tab_switches_field_in_adding() {
        let mut config = Config::new(make_providers(), true, true, false, false);
        config.move_down();
        config.enter();
        if let super::InputStage::Adding { field, .. } = &config.stage {
            assert_eq!(*field, Field::Name);
        } else {
            panic!("expected Adding stage");
        }
        config.tab();
        if let super::InputStage::Adding { field, .. } = &config.stage {
            assert_eq!(*field, Field::Key);
        } else {
            panic!("expected Adding stage");
        }
    }
}
