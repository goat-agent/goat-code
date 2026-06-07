use std::{path::Path, time::Duration};

use crossterm::event::{
    Event as CtEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use futures::StreamExt;
use goat_protocol::{
    Event as EngineEvent, LoginCredential, LoginProvider, ModelEntry, ModelTarget, Op, TaskId,
};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::{Receiver, Sender};

use crate::{
    command::CommandMenu,
    composer::Composer,
    highlight::SyntectHighlighter,
    keymap,
    login::{Login, LoginOutcome},
    picker::{Picker, PickerOutcome},
    theme::Theme,
    transcript::Transcript,
    tui, view,
};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const TICK: Duration = Duration::from_millis(120);
const QUIT_ARM_TICKS: u16 = 25;

enum AppEvent {
    Input(CtEvent),
    Tick,
    Engine(EngineEvent),
    EngineClosed,
}

pub struct App {
    theme: Theme,
    transcript: Transcript,
    composer: Composer,
    highlighter: SyntectHighlighter,
    cwd: String,
    active: Option<TaskId>,
    next_task: u64,
    spinner: usize,
    quit_arm: Option<u16>,
    should_quit: bool,
    dirty: bool,
    scroll: u16,
    follow: bool,
    models: Vec<ModelEntry>,
    model: Option<ModelTarget>,
    picker: Option<Picker>,
    login: Option<Login>,
    login_providers: Vec<LoginProvider>,
    command_menu: Option<CommandMenu>,
    task_start: Option<std::time::Instant>,
}

impl App {
    fn new(theme: Theme) -> Self {
        let cwd = std::env::current_dir()
            .ok()
            .map(|p| shorten_home(&p))
            .unwrap_or_default();
        Self {
            theme,
            transcript: Transcript::default(),
            composer: Composer::default(),
            highlighter: SyntectHighlighter::new(),
            cwd,
            active: None,
            next_task: 1,
            spinner: 0,
            quit_arm: None,
            should_quit: false,
            dirty: true,
            scroll: 0,
            follow: true,
            models: Vec::new(),
            model: None,
            picker: None,
            login: None,
            login_providers: Vec::new(),
            command_menu: None,
            task_start: None,
        }
    }

    fn update(&mut self, event: AppEvent) -> Vec<Op> {
        match event {
            AppEvent::Tick => {
                if self.active.is_some() {
                    self.spinner = self.spinner.wrapping_add(1);
                    self.dirty = true;
                }
                if let Some(ticks) = &mut self.quit_arm {
                    *ticks -= 1;
                    if *ticks == 0 {
                        self.quit_arm = None;
                        self.dirty = true;
                    }
                }
                Vec::new()
            }
            AppEvent::Input(CtEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                self.on_key(key)
            }
            AppEvent::Input(CtEvent::Paste(text)) => {
                if let Some(picker) = &mut self.picker {
                    for ch in text.chars() {
                        picker.on_char(ch);
                    }
                } else if let Some(login) = &mut self.login {
                    login.insert_str(&text);
                } else {
                    self.composer.insert_str(&text);
                }
                self.dirty = true;
                Vec::new()
            }
            AppEvent::Input(CtEvent::Resize(..)) => {
                self.dirty = true;
                Vec::new()
            }
            AppEvent::Input(CtEvent::Mouse(mouse)) => {
                match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        self.follow = false;
                        self.scroll = self.scroll.saturating_sub(3);
                        self.dirty = true;
                    }
                    MouseEventKind::ScrollDown => {
                        self.scroll = self.scroll.saturating_add(3);
                        self.dirty = true;
                    }
                    _ => {}
                }
                Vec::new()
            }
            AppEvent::Input(_) => Vec::new(),
            AppEvent::Engine(event) => {
                self.on_engine(event);
                self.dirty = true;
                Vec::new()
            }
            AppEvent::EngineClosed => {
                self.should_quit = true;
                Vec::new()
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> Vec<Op> {
        tracing::trace!(code = ?key.code, modifiers = ?key.modifiers, "key");
        if self.picker.is_some() {
            return self.on_picker_key(key);
        }
        if self.login.is_some() {
            return self.on_login_key(key);
        }
        if let Some(ch) = keymap::ctrl_key(&key) {
            if ch == 'c' {
                return self.on_ctrl_c();
            }
            self.quit_arm = None;
            self.dirty = true;
            match ch {
                'a' => self.composer.move_home(),
                'e' => self.composer.move_end(),
                'w' => self.composer.delete_word_before(),
                _ => {}
            }
            return Vec::new();
        }
        self.quit_arm = None;
        self.dirty = true;
        match key.code {
            KeyCode::PageUp => {
                self.follow = false;
                self.scroll = self.scroll.saturating_sub(10);
                Vec::new()
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10);
                Vec::new()
            }
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                self.composer.newline();
                Vec::new()
            }
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                self.composer.backspace();
                Vec::new()
            }
            KeyCode::Delete => {
                self.composer.delete_forward();
                Vec::new()
            }
            KeyCode::Left => {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    self.composer.move_word_left();
                } else {
                    self.composer.move_left();
                }
                Vec::new()
            }
            KeyCode::Right => {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    self.composer.move_word_right();
                } else {
                    self.composer.move_right();
                }
                Vec::new()
            }
            KeyCode::Home => {
                self.composer.move_home();
                Vec::new()
            }
            KeyCode::End => {
                self.composer.move_end();
                Vec::new()
            }
            KeyCode::Up => {
                if self.composer.on_first_row() {
                    self.composer.history_prev();
                } else {
                    self.composer.move_up();
                }
                Vec::new()
            }
            KeyCode::Down => {
                if self.composer.on_last_row() {
                    self.composer.history_next();
                } else {
                    self.composer.move_down();
                }
                Vec::new()
            }
            KeyCode::Esc => {
                self.composer.clear();
                Vec::new()
            }
            KeyCode::Char(c) => {
                self.composer.insert_char(c);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn on_picker_key(&mut self, key: KeyEvent) -> Vec<Op> {
        self.dirty = true;
        if let Some(ch) = keymap::ctrl_key(&key) {
            if ch == 'c' {
                self.picker = None;
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Esc => self.picker = None,
            KeyCode::Up => {
                if let Some(picker) = &mut self.picker {
                    picker.move_up();
                }
            }
            KeyCode::Down => {
                if let Some(picker) = &mut self.picker {
                    picker.move_down();
                }
            }
            KeyCode::Backspace => {
                if let Some(picker) = &mut self.picker {
                    picker.backspace();
                }
            }
            KeyCode::Enter => {
                if let Some(picker) = &mut self.picker
                    && let PickerOutcome::Selected(target) = picker.choose()
                {
                    self.picker = None;
                    return vec![Op::SelectModel { target }];
                }
            }
            KeyCode::Char(c) => {
                if let Some(picker) = &mut self.picker {
                    picker.on_char(c);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn on_login_key(&mut self, key: KeyEvent) -> Vec<Op> {
        self.dirty = true;
        if let Some(ch) = keymap::ctrl_key(&key) {
            if ch == 'c' {
                self.login = None;
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Esc => self.login = None,
            KeyCode::Up => {
                if let Some(login) = &mut self.login {
                    login.move_up();
                }
            }
            KeyCode::Down => {
                if let Some(login) = &mut self.login {
                    login.move_down();
                }
            }
            KeyCode::Backspace => {
                if let Some(login) = &mut self.login {
                    login.backspace();
                }
            }
            KeyCode::Enter => {
                if let Some(login) = &mut self.login
                    && let LoginOutcome::Submit {
                        provider,
                        credential,
                    } = login.enter()
                {
                    if matches!(credential, LoginCredential::ApiKey(_)) {
                        self.login = None;
                    }
                    return vec![Op::Login {
                        provider,
                        credential,
                    }];
                }
            }
            KeyCode::Char(c) => {
                if let Some(login) = &mut self.login {
                    login.on_char(c);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn on_ctrl_c(&mut self) -> Vec<Op> {
        self.dirty = true;
        if let Some(id) = self.active {
            return vec![Op::Interrupt { id }];
        }
        if self.quit_arm.is_some() {
            self.should_quit = true;
        } else {
            self.quit_arm = Some(QUIT_ARM_TICKS);
        }
        Vec::new()
    }

    fn submit(&mut self) -> Vec<Op> {
        if self.active.is_some() || self.composer.is_empty() {
            return Vec::new();
        }
        let text = self.composer.take();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        if trimmed == "/model" {
            self.picker = Some(Picker::new(self.models.clone()));
            self.dirty = true;
            return Vec::new();
        }
        if trimmed == "/login" {
            self.login = Some(Login::new(self.login_providers.clone()));
            self.dirty = true;
            return Vec::new();
        }
        if trimmed.starts_with('/') {
            self.dirty = true;
            return Vec::new();
        }
        let id = TaskId(self.next_task);
        self.next_task += 1;
        self.active = Some(id);
        self.transcript.push_user(text.clone());
        self.follow = true;
        vec![Op::SubmitMessage { id, text }]
    }

    fn on_engine(&mut self, event: EngineEvent) {
        match event {
            EngineEvent::TaskStarted { .. } => {
                self.task_start = Some(std::time::Instant::now());
            }
            EngineEvent::ModelListChanged { entries } => {
                if let Some(picker) = &mut self.picker {
                    picker.set_entries(entries.clone());
                }
                self.models = entries;
            }
            EngineEvent::ModelSelected { target } => self.model = Some(target),
            EngineEvent::LoginProviders { providers } => self.login_providers = providers,
            EngineEvent::LoginStatus {
                message, done, ok, ..
            } => {
                if done {
                    self.login = None;
                    if ok {
                        self.transcript.push_notice(message);
                    } else {
                        self.transcript.push_error(message);
                    }
                } else if let Some(login) = &mut self.login {
                    login.set_status(message);
                }
            }
            EngineEvent::TextDelta { chunk, .. } => self.transcript.push_delta(&chunk),
            EngineEvent::TextDone { text, .. } => {
                self.transcript
                    .commit_text(&text, &self.highlighter, self.theme);
            }
            EngineEvent::ToolStarted { call, .. } => self.transcript.push_tool(call),
            EngineEvent::ToolDone { call, outcome, .. } => {
                self.transcript.finish_tool(call, outcome);
            }
            EngineEvent::TaskDone { interrupted, .. } => {
                self.transcript
                    .complete(interrupted, &self.highlighter, self.theme);
                self.active = None;
                self.task_start = None;
            }
            EngineEvent::Error { message, .. } => {
                self.transcript.push_error(message);
                self.active = None;
                self.task_start = None;
            }
        }
        if self.follow {
            self.scroll = u16::MAX;
        }
    }

    pub(crate) fn clamp_scroll(&mut self, viewport_height: u16, content_width: u16) {
        let content = self.transcript.content_height(content_width, self.theme);
        let max = content.saturating_sub(viewport_height);
        self.scroll = self.scroll.min(max);
        if self.scroll >= max {
            self.follow = true;
        }
    }

    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub(crate) fn theme(&self) -> Theme {
        self.theme
    }
    pub(crate) fn transcript(&self) -> &Transcript {
        &self.transcript
    }
    pub(crate) fn composer(&self) -> &Composer {
        &self.composer
    }
    pub(crate) fn composer_height(&self, available_width: u16) -> u16 {
        self.composer.desired_height(available_width)
    }

    pub(crate) fn elapsed_secs(&self) -> Option<u64> {
        self.task_start.map(|t| t.elapsed().as_secs())
    }
    pub(crate) fn is_busy(&self) -> bool {
        self.active.is_some()
    }
    pub(crate) fn cwd(&self) -> &str {
        &self.cwd
    }
    pub(crate) fn quit_armed(&self) -> bool {
        self.quit_arm.is_some()
    }
    pub(crate) fn spinner_frame(&self) -> &'static str {
        SPINNER[self.spinner % SPINNER.len()]
    }

    pub(crate) fn content_height(&self, width: u16) -> u16 {
        self.transcript.content_height(width, self.theme)
    }
    pub(crate) fn scroll(&self) -> u16 {
        self.scroll
    }
    pub(crate) fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref()
    }
    pub(crate) fn login(&self) -> Option<&Login> {
        self.login.as_ref()
    }
    pub(crate) fn command_menu(&self) -> Option<&CommandMenu> {
        self.command_menu.as_ref()
    }
    pub(crate) fn current_model(&self) -> Option<&ModelTarget> {
        self.model.as_ref()
    }
}

fn shorten_home(path: &Path) -> String {
    let display = path.display().to_string();
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if let Some(rest) = display.strip_prefix(home.as_ref()) {
            return format!("~{rest}");
        }
    }
    display
}

pub async fn run(
    ops: Sender<Op>,
    mut events: Receiver<EngineEvent>,
    theme: Theme,
) -> color_eyre::Result<()> {
    let mut terminal = tui::init()?;
    let result = event_loop(&mut terminal, &ops, &mut events, theme).await;
    tui::restore();
    let _ = ops.send(Op::Shutdown).await;
    result
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    ops: &Sender<Op>,
    events: &mut Receiver<EngineEvent>,
    theme: Theme,
) -> color_eyre::Result<()> {
    let mut app = App::new(theme);
    let mut input = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);

    terminal.draw(|frame| view::render(frame, &mut app))?;
    while !app.should_quit {
        let event = tokio::select! {
            maybe = input.next() => match maybe {
                Some(Ok(ev)) => AppEvent::Input(ev),
                Some(Err(_)) | None => break,
            },
            _ = ticker.tick() => AppEvent::Tick,
            maybe = events.recv() => match maybe {
                Some(ev) => AppEvent::Engine(ev),
                None => AppEvent::EngineClosed,
            },
        };

        for op in app.update(event) {
            if ops.send(op).await.is_err() {
                app.should_quit = true;
            }
        }

        if app.take_dirty() {
            terminal.draw(|frame| view::render(frame, &mut app))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use goat_protocol::{
        AccountChoice, AuthMethod, Event as EngineEvent, LoginCredential, LoginProvider,
        ModelEntry, ModelTarget, Op,
    };

    use super::App;
    use crate::theme::Theme;

    fn login_provider(id: &str, method: AuthMethod) -> LoginProvider {
        LoginProvider {
            id: id.to_owned(),
            method,
        }
    }

    fn single_entry(provider: &str, model: &str) -> ModelEntry {
        ModelEntry {
            provider: provider.to_owned(),
            model: model.to_owned(),
            accounts: vec![AccountChoice {
                id: "default".to_owned(),
                display: "default".to_owned(),
                target: ModelTarget {
                    provider: provider.to_owned(),
                    model: model.to_owned(),
                    account: "default".to_owned(),
                },
            }],
        }
    }

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn submit_then_interrupt_emit_ops() {
        let mut app = App::new(Theme::dark());
        app.composer.insert_str("hi");
        let started = app.submit();
        assert!(matches!(started.as_slice(), [Op::SubmitMessage { .. }]));
        let ops = app.on_ctrl_c();
        assert!(matches!(ops.as_slice(), [Op::Interrupt { .. }]));
    }

    #[test]
    fn ctrl_c_when_idle_arms_then_quits() {
        let mut app = App::new(Theme::dark());
        assert!(!app.quit_armed());
        app.on_ctrl_c();
        assert!(app.quit_armed());
        assert!(!app.should_quit);
        app.on_ctrl_c();
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_dubeolsik_arms_then_quits() {
        let mut app = App::new(Theme::dark());
        assert!(!app.quit_armed());
        app.on_key(press(KeyCode::Char('ㅊ'), KeyModifiers::CONTROL));
        assert!(app.quit_armed());
        assert!(!app.should_quit);
        app.on_key(press(KeyCode::Char('ㅊ'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn plain_dubeolsik_inserts_into_composer() {
        let mut app = App::new(Theme::dark());
        app.on_key(press(KeyCode::Char('ㅊ'), KeyModifiers::NONE));
        assert!(!app.composer.is_empty());
    }

    #[test]
    fn ctrl_other_key_does_not_insert() {
        let mut app = App::new(Theme::dark());
        app.on_key(press(KeyCode::Char('ㄴ'), KeyModifiers::CONTROL));
        assert!(app.composer.is_empty());
    }

    #[test]
    fn scroll_follow_resets_on_submit() {
        let mut app = App::new(Theme::dark());
        app.follow = false;
        app.composer.insert_str("hello");
        app.submit();
        assert!(app.follow);
    }

    #[test]
    fn slash_model_opens_picker_without_op() {
        let mut app = App::new(Theme::dark());
        app.composer.insert_str("/model");
        let ops = app.submit();
        assert!(ops.is_empty());
        assert!(app.picker.is_some());
    }

    #[test]
    fn picker_esc_closes() {
        let mut app = App::new(Theme::dark());
        app.composer.insert_str("/model");
        app.submit();
        app.on_key(press(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.picker.is_none());
    }

    #[test]
    fn picker_enter_selects_and_emits_op() {
        let mut app = App::new(Theme::dark());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![single_entry("openai", "gpt")],
        });
        app.composer.insert_str("/model");
        app.submit();
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(ops.as_slice(), [Op::SelectModel { .. }]));
        assert!(app.picker.is_none());
    }

    #[test]
    fn picker_filter_then_select() {
        let mut app = App::new(Theme::dark());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![
                single_entry("openai", "gpt"),
                single_entry("anthropic", "claude"),
            ],
        });
        app.composer.insert_str("/model");
        app.submit();
        for ch in "claude".chars() {
            app.on_key(press(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(ops.as_slice(), [Op::SelectModel { target }] if target.provider == "anthropic")
        );
    }

    #[test]
    fn picker_empty_state_keeps_open_on_enter() {
        let mut app = App::new(Theme::dark());
        app.composer.insert_str("/model");
        app.submit();
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(ops.is_empty());
        assert!(app.picker.is_some());
    }

    #[test]
    fn slash_login_opens_login_overlay() {
        let mut app = App::new(Theme::dark());
        app.on_engine(EngineEvent::LoginProviders {
            providers: vec![login_provider("openai", AuthMethod::ApiKey)],
        });
        app.composer.insert_str("/login");
        let ops = app.submit();
        assert!(ops.is_empty());
        assert!(app.login.is_some());
    }

    #[test]
    fn login_api_key_provider_enters_key_and_emits_op() {
        let mut app = App::new(Theme::dark());
        app.on_engine(EngineEvent::LoginProviders {
            providers: vec![login_provider("openai", AuthMethod::ApiKey)],
        });
        app.composer.insert_str("/login");
        app.submit();
        app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        for ch in "sk".chars() {
            app.on_key(press(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            ops.as_slice(),
            [Op::Login { provider, credential }]
                if provider == "openai" && matches!(credential, LoginCredential::ApiKey(key) if key == "sk")
        ));
        assert!(app.login.is_none());
    }

    #[test]
    fn login_oauth_provider_emits_oauth_and_stays_open() {
        let mut app = App::new(Theme::dark());
        app.on_engine(EngineEvent::LoginProviders {
            providers: vec![login_provider("openai-codex", AuthMethod::OAuth)],
        });
        app.composer.insert_str("/login");
        app.submit();
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            ops.as_slice(),
            [Op::Login { provider, credential }]
                if provider == "openai-codex" && matches!(credential, LoginCredential::OAuth)
        ));
        assert!(app.login.is_some());
    }

    #[test]
    fn unknown_slash_command_is_ignored() {
        let mut app = App::new(Theme::dark());
        app.composer.insert_str("/bogus");
        let ops = app.submit();
        assert!(ops.is_empty());
        assert!(app.picker.is_none());
        assert!(app.login.is_none());
        assert!(app.active.is_none());
    }
}
