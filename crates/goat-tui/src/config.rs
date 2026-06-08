use goat_protocol::{AccountEntry, AuthMethod, LoginCredential};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    overlay::{centered_rect, clamp_u16, overlay_frame, selection_row},
    symbols,
    theme::Theme,
};

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
    error: Option<String>,
}

impl Config {
    pub fn new(providers: Vec<AccountEntry>, dark_theme: bool, mouse_capture: bool) -> Self {
        let mut config = Self {
            section: Section::Providers,
            providers,
            cursor: 0,
            stage: InputStage::List,
            dark_theme,
            mouse_capture,
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
            Section::Appearance => index <= 1,
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
                if self.cursor < 1 {
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
            InputStage::Waiting { .. } => {}
        }
    }

    pub fn enter(&mut self) -> ConfigOutcome {
        self.error = None;
        match &self.stage {
            InputStage::List => self.enter_list(),
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
                    self.stage = InputStage::Adding {
                        provider,
                        method,
                        name: "default".to_owned(),
                        key: String::new(),
                        field: Field::Name,
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
        self.stage = InputStage::List;
    }

    pub fn stage_kind(&self) -> StageKind {
        match self.stage {
            InputStage::List => StageKind::List,
            InputStage::Adding { .. } => StageKind::Input,
            InputStage::Waiting { .. } => StageKind::Waiting,
        }
    }

    pub fn desired_height(&self) -> u16 {
        match self.stage {
            InputStage::List => {
                let rows = self.provider_rows().len();
                let blanks = self.providers.len().saturating_sub(1);
                let content = rows + blanks + 5;
                clamp_u16(content).clamp(10, 30)
            }
            InputStage::Adding { .. } => 9,
            InputStage::Waiting { .. } => 6,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let rect = centered_rect(area, 64, self.desired_height());
        let Some(inner) = overlay_frame(frame, rect, theme, None) else {
            return;
        };
        match &self.stage {
            InputStage::List => {
                let [tabs_area, gap_area, content_area] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(1),
                ])
                .areas(inner);
                let _ = gap_area;
                self.render_tabs(frame, tabs_area, theme);
                self.render_list(frame, content_area, theme);
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
            Span::styled("  ", theme.muted()),
            Span::styled("Providers", providers_style),
            Span::styled("   ", theme.muted()),
            Span::styled("Appearance", appearance_style),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn render_list(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let [body_area, hint_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        match self.section {
            Section::Providers => {
                self.render_providers(frame, body_area, theme);
                render_hint(
                    frame,
                    hint_area,
                    theme,
                    &format!(
                        "  {}{} move   {} add   {} remove   {} section   esc close",
                        symbols::key::ARROW_UP,
                        symbols::key::ARROW_DOWN,
                        symbols::key::RETURN,
                        symbols::key::BACKSPACE,
                        symbols::key::ARROWS_LEFTRIGHT,
                    ),
                );
            }
            Section::Appearance => {
                self.render_appearance(frame, body_area, theme);
                render_hint(
                    frame,
                    hint_area,
                    theme,
                    &format!(
                        "  {}{} move   {} toggle   {} section   esc close",
                        symbols::key::ARROW_UP,
                        symbols::key::ARROW_DOWN,
                        symbols::key::RETURN,
                        symbols::key::ARROWS_LEFTRIGHT,
                    ),
                );
            }
        }
    }

    fn render_providers(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let rows = self.provider_rows();
        let mut lines: Vec<Line> = Vec::new();
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
                    let mut spans = vec![Span::styled(
                        format!("  {}", entry.provider),
                        theme.accent(),
                    )];
                    if entry.local {
                        spans.push(Span::styled("   local", theme.muted()));
                    }
                    lines.push(Line::from(spans));
                }
                RowKind::Account => {
                    let ai = row.account_index.unwrap_or(0);
                    let account = &entry.accounts[ai];
                    let name_style = if selected { theme.key() } else { theme.base() };
                    let left = vec![Span::styled(format!("{:<18}", account.name), name_style)];
                    let right = Some(Span::styled(method_label(account.method), theme.muted()));
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
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn render_appearance(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let lines = vec![
            appearance_row(
                theme,
                self.cursor == 0,
                "theme",
                self.dark_theme,
                "dark",
                "light",
            ),
            appearance_row(
                theme,
                self.cursor == 1,
                "mouse",
                self.mouse_capture,
                "on",
                "off",
            ),
        ];
        frame.render_widget(Paragraph::new(lines), area);
    }
}

fn appearance_row(
    theme: Theme,
    selected: bool,
    label: &str,
    first_active: bool,
    first: &str,
    second: &str,
) -> Line<'static> {
    let marker = if selected { symbols::ui::CARET } else { "  " };
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
    Line::from(vec![
        Span::styled(marker, theme.accent()),
        Span::styled(format!("{label:<12}"), label_style),
        Span::styled(format!("{first_dot} {first:<6}"), first_style),
        Span::styled(format!("{second_dot} {second}"), second_style),
    ])
}

fn method_label(method: AuthMethod) -> &'static str {
    match method {
        AuthMethod::ApiKey => "api key",
        AuthMethod::OAuth => "browser",
        AuthMethod::None => "no auth",
    }
}

fn provider_method(entry: &AccountEntry) -> AuthMethod {
    entry
        .accounts
        .first()
        .map_or(AuthMethod::ApiKey, |a| a.method)
}

fn render_hint(frame: &mut Frame, area: Rect, theme: Theme, text: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text.to_owned(), theme.muted()))),
        area,
    );
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
        Span::styled(format!("  {provider}"), theme.key()),
        Span::styled(
            format!("{} new account", symbols::ui::SEPARATOR),
            theme.muted(),
        ),
    ]);
    frame.render_widget(Paragraph::new(title), title_area);

    let api_key = !matches!(method, AuthMethod::OAuth);
    let name_active = field == Field::Name;
    let name_label_style = if name_active {
        theme.accent()
    } else {
        theme.muted()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("    name   ", name_label_style),
            Span::styled(name.to_owned(), theme.base()),
        ])),
        name_area,
    );
    if name_active {
        place_cursor(frame, name_area, 11, name.chars().count());
    }

    if api_key {
        let key_active = field == Field::Key;
        let key_label_style = if key_active {
            theme.accent()
        } else {
            theme.muted()
        };
        let masked = symbols::ui::MASK.repeat(key.chars().count());
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("    key    ", key_label_style),
                Span::styled(masked, theme.base()),
            ])),
            key_area,
        );
        if key_active {
            place_cursor(frame, key_area, 11, key.chars().count());
        }
    }

    if let Some(message) = error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("    {} {message}", symbols::ui::CROSS),
                theme.error(),
            ))),
            error_area,
        );
    }

    let hint = if api_key {
        format!(
            "  {} save{}{} next field{} esc cancel",
            symbols::key::RETURN,
            symbols::ui::SEPARATOR,
            symbols::key::TAB,
            symbols::ui::SEPARATOR,
        )
    } else {
        format!(
            "  {} open browser{} esc cancel",
            symbols::key::RETURN,
            symbols::ui::SEPARATOR,
        )
    };
    render_hint(frame, hint_area, theme, &hint);
}

fn place_cursor(frame: &mut Frame, area: Rect, prefix: u16, value_len: usize) {
    let col = u16::try_from(value_len)
        .unwrap_or(u16::MAX)
        .saturating_add(prefix);
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
            Span::styled(format!("  {provider}"), theme.key()),
            Span::styled(" \u{b7} ", theme.muted()),
            Span::styled(name.to_owned(), theme.base()),
        ]),
        Line::from(Span::raw("")),
    ];
    if let Some(msg) = status {
        lines.push(Line::from(Span::styled(format!("  {msg}"), theme.muted())));
    }
    frame.render_widget(Paragraph::new(lines), area);
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
            },
            AccountEntry {
                provider: "ollama".to_owned(),
                display_name: "ollama".to_owned(),
                accounts: Vec::new(),
                local: true,
            },
        ]
    }

    #[test]
    fn tab_switches_section() {
        let mut config = Config::new(make_providers(), true, true);
        assert_eq!(config.section, super::Section::Providers);
        config.tab();
        assert_eq!(config.section, super::Section::Appearance);
        config.tab();
        assert_eq!(config.section, super::Section::Providers);
    }

    #[test]
    fn move_down_skips_provider_headers() {
        let config_rows = Config::new(make_providers(), true, true);
        assert_eq!(config_rows.cursor, 1);
        let mut config = config_rows;
        config.move_down();
        assert_eq!(config.cursor, 2);
        config.move_down();
        assert_eq!(config.cursor, 2);
    }

    #[test]
    fn add_account_flow_api_key() {
        let mut config = Config::new(make_providers(), true, true);
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
        let mut config = Config::new(make_providers(), true, true);
        let out = config.remove_selected();
        assert!(matches!(
            out,
            ConfigOutcome::RemoveAccount { ref provider, ref name }
            if provider == "anthropic" && name == "work-key"
        ));
    }

    #[test]
    fn theme_toggle_in_appearance() {
        let mut config = Config::new(make_providers(), true, true);
        config.tab();
        let out = config.enter();
        assert!(matches!(out, ConfigOutcome::SetTheme { dark: false }));
        assert!(!config.dark_theme);
    }

    #[test]
    fn backspace_clears_input() {
        let mut config = Config::new(make_providers(), true, true);
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
        let mut config = Config::new(make_providers(), true, true);
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
