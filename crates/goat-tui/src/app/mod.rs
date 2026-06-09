mod engine;
mod keys;

use std::{collections::HashMap, path::Path, time::Duration};

use crossterm::event::{Event as CtEvent, EventStream, KeyEventKind, MouseEventKind};
use futures::StreamExt;
use goat_commands::{CommandEffect, CommandRegistry};
use goat_protocol::{
    AccountEntry, Effort, Event as EngineEvent, ModelEntry, ModelTarget, Op, RateLimitSnapshot,
    TaskId, Usage,
};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::{Receiver, Sender};

use crate::{
    command::CommandMenu,
    composer::Composer,
    config::{Config, ConfigOutcome},
    highlight::SyntectHighlighter,
    picker::{EffortPicker, Picker, ThreadPicker},
    symbols,
    theme::Theme,
    transcript::Transcript,
    tui,
    usage::UsageView,
    view,
};

pub(crate) enum ResumeIntent {
    Picker,
    Index(usize),
}

pub(crate) struct AgentRunView {
    pub(crate) agent_type: String,
    pub(crate) label: String,
    pub(crate) id: TaskId,
    pub(crate) transcript: Transcript,
    pub(crate) done: Option<bool>,
}

pub(crate) enum MainView {
    Live,
    Agent(TaskId),
}

pub(crate) enum Overlay {
    None,
    Model(Picker),
    Effort(EffortPicker),
    Thread(ThreadPicker),
    Config(Config),
    Commands(CommandMenu),
    Agents(usize),
    Usage,
}

const TICK: Duration = Duration::from_millis(120);
const QUIT_ARM_TICKS: u16 = 25;

pub(crate) enum AppEvent {
    Input(CtEvent),
    Tick,
    Engine(EngineEvent),
    EngineClosed,
}

#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub(crate) theme: Theme,
    pub(crate) transcript: Transcript,
    pub(crate) composer: Composer,
    pub(crate) highlighter: SyntectHighlighter,
    pub(crate) cwd: String,
    pub(crate) active: Option<TaskId>,
    pub(crate) next_task: u64,
    pub(crate) spinner: usize,
    pub(crate) quit_arm: Option<u16>,
    pub(crate) should_quit: bool,
    pub(crate) dirty: bool,
    pub(crate) scroll: u16,
    pub(crate) follow: bool,
    pub(crate) models: Vec<ModelEntry>,
    pub(crate) model: Option<ModelTarget>,
    pub(crate) overlay: Overlay,
    pub(crate) pending_resume: Option<ResumeIntent>,
    pub(crate) account_entries: Vec<AccountEntry>,
    pub(crate) mouse_capture: bool,
    pub(crate) commands: CommandRegistry,
    pub(crate) task_start: Option<std::time::Instant>,
    pub(crate) toasts: Vec<crate::toast::Toast>,
    pub(crate) agent_runs: Vec<AgentRunView>,
    pub(crate) main_view: MainView,
    pub(crate) usage_last: HashMap<(String, String), Usage>,
    pub(crate) usage_total: HashMap<(String, String), (u64, u64)>,
    pub(crate) rate_limits: HashMap<(String, String), (RateLimitSnapshot, i64)>,
    pub(crate) context_window: Option<u32>,
}

impl App {
    pub(crate) fn new(theme: Theme) -> Self {
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
            overlay: Overlay::None,
            pending_resume: None,
            account_entries: Vec::new(),
            mouse_capture: true,
            commands: CommandRegistry::builtin(),
            task_start: None,
            toasts: Vec::new(),
            agent_runs: Vec::new(),
            main_view: MainView::Live,
            usage_last: HashMap::new(),
            usage_total: HashMap::new(),
            rate_limits: HashMap::new(),
            context_window: None,
        }
    }

    pub(crate) fn update(&mut self, event: AppEvent) -> Vec<Op> {
        match event {
            AppEvent::Tick => {
                if self.active.is_some() {
                    self.spinner = self.spinner.wrapping_add(1);
                    self.dirty = true;
                }
                if let Some(ticks) = &mut self.quit_arm {
                    *ticks = ticks.saturating_sub(1);
                    if *ticks == 0 {
                        self.quit_arm = None;
                        self.dirty = true;
                    }
                }
                if crate::toast::tick(&mut self.toasts) {
                    self.dirty = true;
                }
                Vec::new()
            }
            AppEvent::Input(CtEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                self.on_key(key)
            }
            AppEvent::Input(CtEvent::Paste(text)) => {
                match &mut self.overlay {
                    Overlay::Model(picker) => {
                        for ch in text.chars() {
                            picker.on_char(ch);
                        }
                    }
                    Overlay::Config(config) => {
                        for ch in text.chars() {
                            config.on_char(ch);
                        }
                    }
                    _ => {
                        self.composer.insert_str(&text);
                        self.update_command_menu();
                    }
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
                let ops = self.on_engine(event);
                self.dirty = true;
                ops
            }
            AppEvent::EngineClosed => {
                self.should_quit = true;
                Vec::new()
            }
        }
    }

    pub(crate) fn dispatch_slash_command(&mut self, raw: &str) -> Vec<Op> {
        let rest = raw.trim().trim_start_matches('/');
        let (name, args) = match rest.split_once(char::is_whitespace) {
            Some((name, args)) => (name, args.trim()),
            None => (rest, ""),
        };
        let effect = self.commands.resolve(name, args);
        self.apply_command_effect(effect)
    }

    pub(crate) fn apply_command_effect(&mut self, effect: CommandEffect) -> Vec<Op> {
        self.dirty = true;
        match effect {
            CommandEffect::OpenModelPicker => {
                self.overlay = Overlay::Model(Picker::new(self.models.clone(), self.model.clone()));
                Vec::new()
            }
            CommandEffect::SelectModelNamed(query) => self.select_model_named(&query),
            CommandEffect::OpenEffortPicker => {
                let efforts = self.current_efforts();
                if efforts.is_empty() {
                    self.transcript
                        .push_notice("current model has no reasoning effort options".to_owned());
                    return Vec::new();
                }
                let label = self
                    .model
                    .as_ref()
                    .map(|m| format!("{}/{}", m.provider, m.model))
                    .unwrap_or_default();
                let current = self.model.as_ref().and_then(|m| m.effort);
                self.overlay = Overlay::Effort(EffortPicker::new(label, efforts, current));
                Vec::new()
            }
            CommandEffect::SelectEffort(level) => {
                let Some(effort) = Effort::parse(&level) else {
                    self.transcript
                        .push_error(format!("unknown effort: {level}"));
                    return Vec::new();
                };
                if !self.current_efforts().contains(&effort) {
                    self.transcript
                        .push_error(format!("current model does not support effort: {level}"));
                    return Vec::new();
                }
                self.apply_effort(effort)
            }
            CommandEffect::OpenThreadPicker => {
                if self.active.is_some() {
                    self.transcript
                        .push_notice("finish the current task before resuming".to_owned());
                    return Vec::new();
                }
                self.pending_resume = Some(ResumeIntent::Picker);
                vec![Op::ListThreads]
            }
            CommandEffect::ResumeIndex(index) => {
                if self.active.is_some() {
                    self.transcript
                        .push_notice("finish the current task before resuming".to_owned());
                    return Vec::new();
                }
                self.pending_resume = Some(ResumeIntent::Index(index));
                vec![Op::ListThreads]
            }
            CommandEffect::OpenConfig => {
                self.overlay = Overlay::Config(Config::new(
                    self.account_entries.clone(),
                    self.theme.is_dark(),
                    self.mouse_capture,
                ));
                Vec::new()
            }
            CommandEffect::ShowHelp => {
                self.transcript
                    .push_notice(crate::command::help_text(&self.commands));
                Vec::new()
            }
            CommandEffect::RenameConversation(title) => vec![Op::RenameThread { title }],
            CommandEffect::ClearConversation => {
                if self.active.is_some() {
                    return Vec::new();
                }
                self.transcript.clear();
                self.reset_agents();
                self.scroll = 0;
                self.follow = true;
                vec![Op::Clear]
            }
            CommandEffect::Submit(text) => self.submit_text(text),
            CommandEffect::Notice(message) => {
                self.transcript.push_notice(message);
                Vec::new()
            }
            CommandEffect::Error(message) => {
                self.transcript.push_error(message);
                Vec::new()
            }
            CommandEffect::OpenUsage => {
                self.overlay = Overlay::Usage;
                self.dirty = true;
                Vec::new()
            }
            CommandEffect::Noop => Vec::new(),
            CommandEffect::Quit => {
                self.should_quit = true;
                Vec::new()
            }
        }
    }

    pub(crate) fn apply_config_outcome(&mut self, outcome: ConfigOutcome) -> Vec<Op> {
        match outcome {
            ConfigOutcome::Pending => Vec::new(),
            ConfigOutcome::AddAccount {
                provider,
                name,
                credential,
            } => {
                vec![Op::AddAccount {
                    provider,
                    name,
                    credential,
                }]
            }
            ConfigOutcome::RemoveAccount { provider, name } => {
                vec![Op::RemoveAccount { provider, name }]
            }
            ConfigOutcome::SetTheme { dark } => {
                self.theme = if dark { Theme::dark() } else { Theme::light() };
                if let Overlay::Config(config) = &mut self.overlay {
                    config.set_providers(self.account_entries.clone());
                }
                vec![Op::SetTheme { dark }]
            }
            ConfigOutcome::SetMouseCapture { enabled } => {
                self.mouse_capture = enabled;
                Vec::new()
            }
        }
    }

    pub(crate) fn submit(&mut self) -> Vec<Op> {
        if self.active.is_some() || self.composer.is_empty() {
            return Vec::new();
        }
        let text = self.composer.take();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        if trimmed.starts_with('/') {
            let cmd = trimmed.to_owned();
            return self.dispatch_slash_command(&cmd);
        }
        self.submit_text(text)
    }

    pub(crate) fn submit_text(&mut self, text: String) -> Vec<Op> {
        if self.active.is_some() {
            return Vec::new();
        }
        let id = TaskId(self.next_task);
        self.next_task += 1;
        self.active = Some(id);
        self.reset_agents();
        self.transcript.push_user(text.clone());
        self.follow = true;
        vec![Op::SubmitMessage { id, text }]
    }

    pub(crate) fn current_efforts(&self) -> Vec<Effort> {
        let Some(model) = &self.model else {
            return Vec::new();
        };
        self.models
            .iter()
            .find(|entry| entry.provider == model.provider && entry.model == model.model)
            .map(|entry| entry.efforts.clone())
            .unwrap_or_default()
    }

    pub(crate) fn apply_effort(&mut self, effort: Effort) -> Vec<Op> {
        let Some(current) = &self.model else {
            self.transcript
                .push_error("select a model first".to_owned());
            return Vec::new();
        };
        let mut target = current.clone();
        target.effort = Some(effort);
        vec![Op::SelectModel { target }]
    }

    pub(crate) fn select_model_named(&mut self, query: &str) -> Vec<Op> {
        let needle = query.trim().to_lowercase();
        let exact: Vec<&ModelEntry> = self
            .models
            .iter()
            .filter(|entry| {
                entry.model.to_lowercase() == needle
                    || format!("{}/{}", entry.provider, entry.model).to_lowercase() == needle
            })
            .collect();
        if let [entry] = exact.as_slice()
            && let [account] = entry.accounts.as_slice()
        {
            return vec![Op::SelectModel {
                target: account.target.clone(),
            }];
        }
        let mut picker = Picker::new(self.models.clone(), self.model.clone());
        for ch in query.trim().chars() {
            picker.on_char(ch);
        }
        self.overlay = Overlay::Model(picker);
        Vec::new()
    }

    pub(crate) fn update_command_menu(&mut self) {
        let text = self.composer.text();
        let trimmed = text.trim_start();
        if trimmed.starts_with('/') && !trimmed.contains(' ') {
            match &mut self.overlay {
                Overlay::Commands(menu) => menu.update(&self.commands, trimmed),
                _ => {
                    self.overlay = Overlay::Commands(CommandMenu::new(&self.commands, trimmed));
                }
            }
        } else if matches!(self.overlay, Overlay::Commands(_)) {
            self.overlay = Overlay::None;
        }
    }

    pub(crate) fn clamp_scroll(&mut self, viewport_height: u16, content_width: u16) {
        let max = self
            .content_height(content_width)
            .saturating_sub(viewport_height);
        if self.scroll > max {
            self.scroll = max;
        }
        self.follow = self.scroll >= max;
    }

    pub(crate) fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub(crate) fn theme(&self) -> Theme {
        self.theme
    }
    pub(crate) fn transcript(&self) -> &Transcript {
        self.active_transcript()
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
        symbols::SPINNER[self.spinner % symbols::SPINNER.len()]
    }

    pub(crate) fn content_height(&self, width: u16) -> u16 {
        self.active_transcript().content_height(width, self.theme)
    }
    pub(crate) fn scroll(&self) -> u16 {
        self.scroll
    }
    pub(crate) fn overlay(&self) -> &Overlay {
        &self.overlay
    }
    pub(crate) fn follow(&self) -> bool {
        self.follow
    }
    pub(crate) fn current_model(&self) -> Option<&ModelTarget> {
        self.model.as_ref()
    }

    pub(crate) fn provider_has_multiple_accounts(&self, provider: &str) -> bool {
        self.account_entries
            .iter()
            .find(|e| e.provider == provider)
            .is_some_and(|e| e.accounts.len() > 1)
    }
    pub(crate) fn toasts(&self) -> &[crate::toast::Toast] {
        &self.toasts
    }

    pub(crate) fn reset_agents(&mut self) {
        self.agent_runs.clear();
        self.main_view = MainView::Live;
        if matches!(self.overlay, Overlay::Agents(_)) {
            self.overlay = Overlay::None;
        }
    }

    pub(crate) fn active_transcript(&self) -> &Transcript {
        match self.main_view {
            MainView::Live => &self.transcript,
            MainView::Agent(id) => self
                .agent_runs
                .iter()
                .find(|run| run.id == id)
                .map_or(&self.transcript, |run| &run.transcript),
        }
    }

    pub(crate) fn set_agent_cursor(&mut self, cursor: usize) {
        if let Some(run) = self.agent_runs.get(cursor) {
            self.overlay = Overlay::Agents(cursor);
            self.main_view = MainView::Agent(run.id);
            self.scroll = 0;
            self.follow = true;
            self.dirty = true;
        }
    }

    pub(crate) fn close_agent_selector(&mut self) {
        self.overlay = Overlay::None;
        self.main_view = MainView::Live;
        self.follow = true;
        self.scroll = u16::MAX;
        self.dirty = true;
    }

    pub(crate) fn agent_runs(&self) -> &[AgentRunView] {
        &self.agent_runs
    }
    pub(crate) fn agent_selector(&self) -> Option<usize> {
        match self.overlay {
            Overlay::Agents(cursor) => Some(cursor),
            _ => None,
        }
    }
    pub(crate) fn agent_status(&self) -> Option<String> {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for run in self.agent_runs.iter().filter(|run| run.done.is_none()) {
            if let Some(entry) = counts.iter_mut().find(|(kind, _)| *kind == run.agent_type) {
                entry.1 += 1;
            } else {
                counts.push((run.agent_type.as_str(), 1));
            }
        }
        let running: usize = counts.iter().map(|(_, n)| n).sum();
        if running == 0 {
            return None;
        }
        let parts: Vec<String> = counts
            .iter()
            .map(|(kind, n)| format!("{n} {kind}"))
            .collect();
        Some(format!("{running} agents · {}", parts.join(", ")))
    }

    pub(crate) fn build_usage_view(&self) -> UsageView {
        UsageView::new(
            self.account_entries.clone(),
            self.usage_last.clone(),
            self.usage_total.clone(),
            self.rate_limits.clone(),
            self.context_window,
            self.model.clone(),
        )
    }

    pub(crate) fn ctx_indicator(&self) -> Option<(f32, u64, u32)> {
        let model = self.model.as_ref()?;
        let window = self.context_window?;
        let key = (model.provider.clone(), model.account.clone());
        let usage = self.usage_last.get(&key)?;
        let used = u64::from(usage.input_tokens) + u64::from(usage.output_tokens);
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let pct = (used as f64 / f64::from(window) * 100.0).min(100.0) as f32;
        Some((pct, used, window))
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
    use goat_protocol::{AccountChoice, Event as EngineEvent, ModelEntry, ModelTarget, Op, TaskId};

    use super::{App, Overlay};
    use crate::theme::Theme;

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
                    effort: None,
                },
            }],
            context_window: None,
            efforts: Vec::new(),
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
    fn clear_command_empties_transcript_and_emits_clear() {
        let mut app = App::new(Theme::dark());
        app.transcript.push_user("earlier message");
        app.scroll = 9;
        app.follow = false;
        app.composer.insert_str("/clear");
        let ops = app.submit();
        assert!(matches!(ops.as_slice(), [Op::Clear]));
        assert!(app.transcript.items.is_empty());
        assert_eq!(app.scroll, 0);
        assert!(app.follow);
    }

    #[test]
    fn clear_command_ignored_while_active() {
        let mut app = App::new(Theme::dark());
        app.active = Some(TaskId(1));
        app.transcript.push_user("in flight");
        let ops = app.dispatch_slash_command("/clear");
        assert!(ops.is_empty());
        assert!(!app.transcript.items.is_empty());
    }

    #[test]
    fn slash_model_opens_picker_without_op() {
        let mut app = App::new(Theme::dark());
        app.composer.insert_str("/model");
        let ops = app.submit();
        assert!(ops.is_empty());
        assert!(matches!(app.overlay, Overlay::Model(_)));
    }

    #[test]
    fn picker_esc_closes() {
        let mut app = App::new(Theme::dark());
        app.composer.insert_str("/model");
        app.submit();
        app.on_key(press(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.overlay, Overlay::None));
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
        assert!(matches!(app.overlay, Overlay::None));
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
        assert!(matches!(app.overlay, Overlay::Model(_)));
    }

    #[test]
    fn unknown_slash_command_pushes_error() {
        let mut app = App::new(Theme::dark());
        app.composer.insert_str("/bogus");
        let ops = app.submit();
        assert!(ops.is_empty());
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.active.is_none());
        assert_eq!(app.transcript.items.len(), 1);
        assert!(matches!(
            &app.transcript.items[0],
            crate::transcript::Item::Error(_)
        ));
    }

    #[test]
    fn slash_help_pushes_notice() {
        let mut app = App::new(Theme::dark());
        app.composer.insert_str("/help");
        let ops = app.submit();
        assert!(ops.is_empty());
        assert!(app.active.is_none());
        assert_eq!(app.transcript.items.len(), 1);
        assert!(matches!(
            &app.transcript.items[0],
            crate::transcript::Item::Notice(_)
        ));
    }

    #[test]
    fn skills_changed_registers_invokable_command() {
        let mut app = App::new(Theme::dark());
        app.on_engine(EngineEvent::SkillsChanged {
            skills: vec![goat_protocol::SkillInfo {
                name: "demo".to_owned(),
                description: "a demo".to_owned(),
            }],
        });
        app.composer.insert_str("/demo");
        let ops = app.submit();
        assert!(matches!(ops.as_slice(), [Op::SubmitMessage { .. }]));
        assert!(app.active.is_some());
    }

    #[test]
    fn unknown_skill_command_pushes_error() {
        let mut app = App::new(Theme::dark());
        app.composer.insert_str("/demo");
        let ops = app.submit();
        assert!(ops.is_empty());
        assert_eq!(app.transcript.items.len(), 1);
        assert!(matches!(
            &app.transcript.items[0],
            crate::transcript::Item::Error(_)
        ));
    }

    fn entry_with_efforts(
        provider: &str,
        model: &str,
        efforts: Vec<goat_protocol::Effort>,
    ) -> ModelEntry {
        let mut entry = single_entry(provider, model);
        entry.efforts = efforts;
        entry
    }

    fn select_model(app: &mut App, provider: &str, model: &str) {
        app.on_engine(EngineEvent::ModelSelected {
            target: ModelTarget {
                provider: provider.to_owned(),
                model: model.to_owned(),
                account: "default".to_owned(),
                effort: None,
            },
        });
    }

    #[test]
    fn effort_without_model_notices() {
        let mut app = App::new(Theme::dark());
        let ops = app.dispatch_slash_command("/effort");
        assert!(ops.is_empty());
        assert!(!matches!(app.overlay, Overlay::Effort(_)));
        assert!(matches!(
            app.transcript.items.last(),
            Some(crate::transcript::Item::Notice(_))
        ));
    }

    #[test]
    fn effort_picker_opens_and_selects() {
        use goat_protocol::Effort;
        let mut app = App::new(Theme::dark());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![entry_with_efforts(
                "openai",
                "gpt",
                vec![Effort::Low, Effort::High],
            )],
        });
        select_model(&mut app, "openai", "gpt");
        let ops = app.dispatch_slash_command("/effort");
        assert!(ops.is_empty());
        assert!(matches!(app.overlay, Overlay::Effort(_)));
        app.on_key(press(KeyCode::Down, KeyModifiers::NONE));
        let ops = app.on_key(press(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(ops.as_slice(), [Op::SelectModel { target }] if target.effort == Some(Effort::High))
        );
        assert!(!matches!(app.overlay, Overlay::Effort(_)));
    }

    #[test]
    fn effort_arg_sets_supported_level() {
        use goat_protocol::Effort;
        let mut app = App::new(Theme::dark());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![entry_with_efforts(
                "openai",
                "gpt",
                vec![Effort::Low, Effort::Medium, Effort::High],
            )],
        });
        select_model(&mut app, "openai", "gpt");
        let ops = app.dispatch_slash_command("/effort high");
        assert!(
            matches!(ops.as_slice(), [Op::SelectModel { target }] if target.effort == Some(Effort::High))
        );
    }

    #[test]
    fn effort_arg_rejects_unsupported_level() {
        use goat_protocol::Effort;
        let mut app = App::new(Theme::dark());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![entry_with_efforts("openai", "gpt", vec![Effort::Low])],
        });
        select_model(&mut app, "openai", "gpt");
        let ops = app.dispatch_slash_command("/effort max");
        assert!(ops.is_empty());
        assert!(matches!(
            app.transcript.items.last(),
            Some(crate::transcript::Item::Error(_))
        ));
    }

    #[test]
    fn model_arg_selects_unique_match() {
        let mut app = App::new(Theme::dark());
        app.on_engine(EngineEvent::ModelListChanged {
            entries: vec![
                single_entry("openai", "gpt"),
                single_entry("anthropic", "claude"),
            ],
        });
        let ops = app.dispatch_slash_command("/model claude");
        assert!(matches!(ops.as_slice(), [Op::SelectModel { target }] if target.model == "claude"));
        assert!(!matches!(app.overlay, Overlay::Model(_)));
    }

    #[test]
    fn resume_requests_list_then_opens_picker() {
        use goat_protocol::ThreadSummary;
        let mut app = App::new(Theme::dark());
        let ops = app.dispatch_slash_command("/resume");
        assert!(matches!(ops.as_slice(), [Op::ListThreads]));
        let ops = app.on_engine(EngineEvent::ThreadsListed {
            threads: vec![ThreadSummary {
                id: 7,
                title: "first chat".to_owned(),
                model: "openai/gpt".to_owned(),
                updated_at: 1,
            }],
        });
        assert!(ops.is_empty());
        assert!(matches!(app.overlay, Overlay::Thread(_)));
    }

    #[test]
    fn resume_index_resolves_to_resume_op() {
        use goat_protocol::ThreadSummary;
        let mut app = App::new(Theme::dark());
        let ops = app.dispatch_slash_command("/resume 1");
        assert!(matches!(ops.as_slice(), [Op::ListThreads]));
        let ops = app.on_engine(EngineEvent::ThreadsListed {
            threads: vec![ThreadSummary {
                id: 42,
                title: "chat".to_owned(),
                model: "openai/gpt".to_owned(),
                updated_at: 1,
            }],
        });
        assert!(matches!(ops.as_slice(), [Op::Resume { thread_id: 42 }]));
        assert!(!matches!(app.overlay, Overlay::Thread(_)));
    }

    #[test]
    fn conversation_restored_rebuilds_transcript() {
        use goat_protocol::{ToolCall, ToolCallId, ToolOutcome, TranscriptEntry};
        let mut app = App::new(Theme::dark());
        app.transcript.push_user("stale");
        app.on_engine(EngineEvent::ConversationRestored {
            target: ModelTarget {
                provider: "anthropic".to_owned(),
                model: "claude".to_owned(),
                account: "default".to_owned(),
                effort: Some(goat_protocol::Effort::High),
            },
            entries: vec![
                TranscriptEntry::User("hello".to_owned()),
                TranscriptEntry::Assistant("hi there".to_owned()),
                TranscriptEntry::Tool {
                    call: ToolCall {
                        id: ToolCallId(1),
                        name: "Read".to_owned(),
                        input: "f.rs".to_owned(),
                    },
                    outcome: ToolOutcome {
                        ok: true,
                        summary: Some("done".to_owned()),
                    },
                },
            ],
        });
        assert_eq!(app.transcript.items.len(), 3);
        assert!(matches!(
            &app.transcript.items[0],
            crate::transcript::Item::User(_)
        ));
        assert!(matches!(
            &app.transcript.items[2],
            crate::transcript::Item::Tool { .. }
        ));
        assert_eq!(
            app.current_model().and_then(|m| m.effort),
            Some(goat_protocol::Effort::High)
        );
    }

    #[test]
    fn agent_events_route_and_drill_in() {
        use goat_protocol::{ToolCall, ToolCallId, ToolOutcome};
        let mut app = App::new(Theme::dark());
        app.composer.insert_str("go");
        app.submit();
        let top = app.active.unwrap();
        app.on_engine(EngineEvent::ToolStarted {
            id: top,
            call: ToolCall {
                id: ToolCallId(1),
                name: "Agent".to_owned(),
                input: "{}".to_owned(),
            },
        });
        let child = TaskId(1 << 32);
        app.on_engine(EngineEvent::AgentStarted {
            id: child,
            parent: top,
            agent_type: "explore".to_owned(),
            label: "look into it".to_owned(),
        });
        assert_eq!(app.agent_runs().len(), 1);
        app.on_engine(EngineEvent::ToolStarted {
            id: child,
            call: ToolCall {
                id: ToolCallId(1),
                name: "Grep".to_owned(),
                input: "x".to_owned(),
            },
        });
        app.on_engine(EngineEvent::ToolDone {
            id: child,
            call: ToolCallId(1),
            outcome: ToolOutcome {
                ok: true,
                summary: None,
            },
        });

        assert_eq!(app.transcript.items.len(), 2);
        assert_eq!(app.agent_runs[0].transcript.items.len(), 1);
        assert!(app.agent_status().is_some_and(|s| s.contains("explore")));

        app.on_engine(EngineEvent::AgentDone {
            id: child,
            ok: true,
        });
        assert_eq!(app.agent_runs[0].done, Some(true));
        assert!(app.agent_status().is_none());

        assert_eq!(app.transcript().items.len(), 2);
        app.set_agent_cursor(0);
        assert!(matches!(app.main_view, super::MainView::Agent(_)));
        assert_eq!(app.transcript().items.len(), 1);
        app.close_agent_selector();
        assert!(matches!(app.main_view, super::MainView::Live));
        assert_eq!(app.transcript().items.len(), 2);
    }
}
