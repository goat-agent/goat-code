use std::{
    collections::HashSet,
    fmt::Write as _,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use goat_auth::{CredentialKey, CredentialKind, CredentialStore, ResolvedCredential, SecretString};
use goat_core::Engine;
use goat_protocol::{
    AccountChoice, AccountEntry, AccountInfo, AuthMethod, Effort, Event, LoginCredential,
    LoginProvider, ModelEntry, ModelTarget, NotifyKind, Op, SkillInfo, TaskId, ThreadSummary,
    ToolCall, ToolCallId, ToolOutcome, TranscriptEntry,
};
use goat_provider::{
    ContentBlock, MessageRole, ModelEvent, ModelProvider, ModelRequest, ProviderMessage,
    ToolDefinition,
};
use goat_providers::{DEFAULT_ACCOUNT, Registry};
use goat_store::{NewMessage, NewThread, NewToolCall, NewTurn, Store};
use goat_tool::ToolContext;
use goat_tools::ToolRegistry;
use tokio::{
    sync::{Semaphore, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

mod agent;

pub use agent::{AgentRegistry, AgentSpec, ToolSelection};

const MAX_TOOL_ROUNDS: usize = 20;
const MAX_CONCURRENT_AGENTS: usize = 8;
const AGENT_TOOL_NAME: &str = "Agent";
const CHILD_ID_BASE: u64 = 1 << 32;
const SYSTEM_PROMPT: &str = "You are Goat, an expert software engineering assistant. You help users understand, build, and improve software by reading code, running tools, and providing accurate, actionable guidance. When using tools, prefer targeted reads and searches over broad exploration. Always verify your understanding before making changes.";

fn build_system_prompt(skills: &[SkillInfo]) -> String {
    if skills.is_empty() {
        return SYSTEM_PROMPT.to_owned();
    }
    let mut prompt = String::from(SYSTEM_PROMPT);
    prompt.push_str(
        "\n\nAvailable skills. Call the Skill tool with a skill's name to load its full instructions before following it:",
    );
    for skill in skills {
        let _ = write!(prompt, "\n- {}: {}", skill.name, skill.description);
    }
    prompt
}

fn load_skill_infos(cwd: &std::path::Path) -> Vec<SkillInfo> {
    goat_skill::load(cwd)
        .iter()
        .map(|skill| SkillInfo {
            name: skill.name.clone(),
            description: skill.description.clone(),
        })
        .collect()
}

pub struct GoatAgent {
    registry: Registry,
    tools: ToolRegistry,
    store: Store,
    credentials: CredentialStore,
    target: Option<ModelTarget>,
}

impl GoatAgent {
    pub fn new(
        registry: Registry,
        store: Store,
        credentials: CredentialStore,
        target: Option<ModelTarget>,
    ) -> Self {
        Self {
            registry,
            tools: ToolRegistry::builtin(),
            store,
            credentials,
            target,
        }
    }
}

impl Engine for GoatAgent {
    fn spawn(self, ops: mpsc::Receiver<Op>, events: mpsc::Sender<Event>) -> JoinHandle<()> {
        tokio::spawn(run(self, ops, events))
    }
}

struct Ctx<'a> {
    registry: &'a Registry,
    credentials: &'a CredentialStore,
    tools: &'a ToolRegistry,
    agents: &'a AgentRegistry,
    store: &'a Store,
    events: &'a mpsc::Sender<Event>,
    skills: &'a [SkillInfo],
    semaphore: &'a Arc<Semaphore>,
    child_ids: &'a AtomicU64,
}

enum Flow {
    Continue,
    Shutdown,
}

struct TurnIds {
    stored_thread: Option<i64>,
    turn_db_id: Option<i64>,
}

enum Report<'a> {
    Top(&'a TurnIds),
    Child,
}

struct Run<'a> {
    id: TaskId,
    report: Report<'a>,
}

impl<'a> Run<'a> {
    fn top(id: TaskId, ids: &'a TurnIds) -> Self {
        Self {
            id,
            report: Report::Top(ids),
        }
    }

    fn child(id: TaskId) -> Self {
        Self {
            id,
            report: Report::Child,
        }
    }

    fn ids(&self) -> Option<&TurnIds> {
        match &self.report {
            Report::Top(ids) => Some(ids),
            Report::Child => None,
        }
    }
}

struct LoopEnv<'a> {
    provider: &'a dyn ModelProvider,
    target: &'a ModelTarget,
    tool_defs: &'a [ToolDefinition],
    cwd: &'a Path,
    allow_delegate: bool,
}

enum RoundEnd {
    Completed,
    Cancelled,
    Failed(String),
}

struct RoundResult {
    end: RoundEnd,
    raw: String,
    thinking: Option<(String, String)>,
    redacted: Vec<String>,
    pending_calls: Vec<(String, String, String)>,
}

struct ToolExecResult {
    result_content: ContentBlock,
    cancelled: bool,
}

struct ToolBatchResult {
    tool_results: Vec<ContentBlock>,
    cancelled: bool,
}

struct Prepared<'a> {
    vendor_id: &'a str,
    name: &'a str,
    input_json: &'a str,
    tui_id: u64,
    db_id: Option<i64>,
}

enum RoundOutcome {
    Done,
    Continue,
    Cancelled,
}

enum LoopOutcome {
    Completed,
    Cancelled,
    Failed(String),
}

enum TurnEnd {
    Done,
    Interrupted,
    Failed(String),
    Shutdown,
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
        |_| {
            tracing::warn!("system clock before unix epoch");
            0
        },
        |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
    )
}

async fn restore_target(store: &Store, credentials: &CredentialStore) -> Option<ModelTarget> {
    let cwd = std::env::current_dir()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let thread = store.latest_thread_in(cwd).await.ok().flatten()?;
    let provider = goat_providers::build_provider(credentials, &thread.provider, &thread.account)?;
    if !provider.authenticated() {
        return None;
    }
    Some(ModelTarget {
        provider: thread.provider,
        model: thread.model,
        account: thread.account,
        effort: thread.effort.as_deref().and_then(Effort::parse),
    })
}

fn effort_string(effort: Option<Effort>) -> Option<String> {
    effort.map(|e| e.as_str().to_owned())
}

fn parse_content_blocks(body: &str) -> Vec<ContentBlock> {
    serde_json::from_str::<Vec<ContentBlock>>(body).unwrap_or_else(|_| {
        vec![ContentBlock::Text {
            text: body.to_owned(),
        }]
    })
}

async fn emit_accounts_changed(
    events: &mpsc::Sender<Event>,
    registry: &Registry,
    credentials: &CredentialStore,
) {
    let _ = events
        .send(Event::AccountsChanged {
            providers: build_account_entries(registry, credentials),
        })
        .await;
}

async fn handle_remove_account(
    provider: String,
    name: String,
    credentials: &CredentialStore,
    registry: &mut Registry,
    events: &mpsc::Sender<Event>,
) {
    let key = CredentialKey {
        provider: provider.clone(),
        account: name.clone(),
    };
    if let Err(err) = credentials.remove(&key) {
        tracing::warn!(%err, "failed to remove account");
    }
    *registry = Registry::for_account(credentials, DEFAULT_ACCOUNT);
    let entries = discover_ready(registry, credentials).await;
    let _ = events.send(Event::ModelListChanged { entries }).await;
    emit_accounts_changed(events, registry, credentials).await;
}

#[allow(clippy::too_many_lines)]
async fn run(agent: GoatAgent, mut ops: mpsc::Receiver<Op>, events: mpsc::Sender<Event>) {
    let GoatAgent {
        mut registry,
        tools,
        store,
        credentials,
        mut target,
    } = agent;
    let mut history: Vec<ProviderMessage> = Vec::new();
    let mut thread_id: Option<i64> = None;

    if target.is_none() {
        target = restore_target(&store, &credentials).await;
    }
    announce_startup(&events, &registry, &credentials, target.as_ref()).await;

    let cwd = std::env::current_dir().unwrap_or_default();
    let skills = load_skill_infos(&cwd);
    let agents = AgentRegistry::load(&cwd);
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_AGENTS));
    let child_ids = AtomicU64::new(CHILD_ID_BASE);
    let _ = events
        .send(Event::SkillsChanged {
            skills: skills.clone(),
        })
        .await;

    while let Some(op) = ops.recv().await {
        match op {
            Op::SubmitMessage { id, text } => {
                let ctx = Ctx {
                    registry: &registry,
                    credentials: &credentials,
                    tools: &tools,
                    agents: &agents,
                    store: &store,
                    events: &events,
                    skills: &skills,
                    semaphore: &semaphore,
                    child_ids: &child_ids,
                };
                if let Flow::Shutdown = handle_turn(
                    &ctx,
                    id,
                    text,
                    &mut target,
                    &mut history,
                    &mut thread_id,
                    &mut ops,
                )
                .await
                {
                    break;
                }
            }
            Op::Interrupt { .. } | Op::SetTheme { .. } => {}
            Op::Clear => {
                history.clear();
                thread_id = None;
            }
            Op::SelectModel { .. } => {
                handle_idle_op(op, &store, thread_id, &mut target, &events).await;
            }
            Op::Login {
                provider,
                credential,
            } => {
                let ctx = LoginCtx {
                    credentials: &credentials,
                    registry: &mut registry,
                    events: &events,
                };
                handle_login(ctx, provider, DEFAULT_ACCOUNT.to_owned(), credential, false).await;
            }
            Op::AddAccount {
                provider,
                name,
                credential,
            } => {
                let ctx = LoginCtx {
                    credentials: &credentials,
                    registry: &mut registry,
                    events: &events,
                };
                handle_login(ctx, provider, name, credential, true).await;
            }
            Op::RemoveAccount { provider, name } => {
                handle_remove_account(provider, name, &credentials, &mut registry, &events).await;
            }
            Op::ListThreads => {
                handle_list_threads(&store, &events).await;
            }
            Op::Resume { thread_id: tid } => {
                handle_resume(
                    &store,
                    &skills,
                    tid,
                    &mut target,
                    &mut history,
                    &mut thread_id,
                    &events,
                )
                .await;
            }
            Op::Shutdown => break,
        }
    }
}

fn build_account_entries(registry: &Registry, credentials: &CredentialStore) -> Vec<AccountEntry> {
    let stored = credentials.entries();
    registry
        .login_providers()
        .into_iter()
        .map(|(provider_id, auth_method)| {
            let is_local = matches!(auth_method, AuthMethod::None);
            let accounts = stored
                .iter()
                .filter(|(key, _)| key.provider == provider_id)
                .map(|(key, kind)| AccountInfo {
                    name: key.account.clone(),
                    method: match kind {
                        CredentialKind::ApiKey => AuthMethod::ApiKey,
                        CredentialKind::OAuth => AuthMethod::OAuth,
                    },
                })
                .collect();
            AccountEntry {
                display_name: provider_id.clone(),
                provider: provider_id,
                accounts,
                local: is_local,
            }
        })
        .collect()
}

async fn announce_startup(
    events: &mpsc::Sender<Event>,
    registry: &Registry,
    credentials: &CredentialStore,
    target: Option<&ModelTarget>,
) {
    let _ = events
        .send(Event::LoginProviders {
            providers: registry
                .login_providers()
                .into_iter()
                .map(|(id, method)| LoginProvider { id, method })
                .collect(),
        })
        .await;
    let _ = events
        .send(Event::AccountsChanged {
            providers: build_account_entries(registry, credentials),
        })
        .await;
    let _ = events
        .send(Event::ModelListChanged {
            entries: catalog_only(registry, credentials),
        })
        .await;
    let entries = discover_ready(registry, credentials).await;
    let _ = events.send(Event::ModelListChanged { entries }).await;
    if let Some(selected) = target {
        let _ = events
            .send(Event::ModelSelected {
                target: selected.clone(),
            })
            .await;
    }
}

struct LoginCtx<'a> {
    credentials: &'a CredentialStore,
    registry: &'a mut Registry,
    events: &'a mpsc::Sender<Event>,
}

async fn login_succeeded(provider: &str, events: &mpsc::Sender<Event>) {
    let _ = events
        .send(Event::Notify {
            kind: NotifyKind::Success,
            message: format!("{provider} connected"),
        })
        .await;
    let _ = events
        .send(Event::LoginStatus {
            provider: provider.to_owned(),
            message: String::new(),
            done: true,
            ok: true,
        })
        .await;
}

async fn login_failed(provider: &str, events: &mpsc::Sender<Event>, message: String) {
    let _ = events
        .send(Event::LoginStatus {
            provider: provider.to_owned(),
            message,
            done: true,
            ok: false,
        })
        .await;
}

async fn store_credential(
    ctx: &mut LoginCtx<'_>,
    provider: &str,
    key: &CredentialKey,
    credential: LoginCredential,
) -> Result<(), String> {
    match credential {
        LoginCredential::ApiKey(secret) => {
            let resolved = ResolvedCredential::ApiKey(SecretString::from(secret));
            ctx.credentials
                .store(key, resolved)
                .map_err(|err| err.to_string())
        }
        LoginCredential::OAuth => {
            let (status_tx, mut status_rx) = mpsc::channel::<String>(8);
            let status_provider = provider.to_owned();
            let status_events = ctx.events.clone();
            let forwarder = tokio::spawn(async move {
                while let Some(message) = status_rx.recv().await {
                    let _ = status_events
                        .send(Event::LoginStatus {
                            provider: status_provider.clone(),
                            message,
                            done: false,
                            ok: false,
                        })
                        .await;
                }
            });
            let result = goat_providers::oauth_login(provider, &status_tx).await;
            drop(status_tx);
            let _ = forwarder.await;
            match result {
                Ok(tokens) => {
                    let resolved = ResolvedCredential::OAuth(tokens);
                    ctx.credentials
                        .store(key, resolved)
                        .map_err(|err| err.to_string())
                }
                Err(err) => Err(err.to_string()),
            }
        }
    }
}

async fn validate_stored(
    credentials: &CredentialStore,
    provider: &str,
    name: &str,
) -> Result<(), String> {
    match goat_providers::build_provider(credentials, provider, name) {
        Some(target) => target
            .validate()
            .await
            .unwrap_or_else(|err| Err(err.to_string())),
        None => Err("unknown provider".to_owned()),
    }
}

async fn handle_login(
    mut ctx: LoginCtx<'_>,
    provider: String,
    name: String,
    credential: LoginCredential,
    dedup: bool,
) {
    let key = CredentialKey {
        provider: provider.clone(),
        account: name.clone(),
    };
    if dedup
        && ctx
            .credentials
            .entries()
            .iter()
            .any(|(stored, _)| stored == &key)
    {
        login_failed(
            &provider,
            ctx.events,
            format!("account '{name}' already exists"),
        )
        .await;
        return;
    }
    if let Err(message) = store_credential(&mut ctx, &provider, &key, credential).await {
        login_failed(&provider, ctx.events, message).await;
        emit_accounts_changed(ctx.events, ctx.registry, ctx.credentials).await;
        return;
    }
    *ctx.registry = Registry::for_account(ctx.credentials, DEFAULT_ACCOUNT);
    if let Err(message) = validate_stored(ctx.credentials, &provider, &name).await {
        let _ = ctx.credentials.remove(&key);
        *ctx.registry = Registry::for_account(ctx.credentials, DEFAULT_ACCOUNT);
        login_failed(&provider, ctx.events, message).await;
        emit_accounts_changed(ctx.events, ctx.registry, ctx.credentials).await;
        return;
    }
    let entries = discover_ready(ctx.registry, ctx.credentials).await;
    let _ = ctx.events.send(Event::ModelListChanged { entries }).await;
    login_succeeded(&provider, ctx.events).await;
    emit_accounts_changed(ctx.events, ctx.registry, ctx.credentials).await;
}

fn account_names_for(credentials: &CredentialStore, provider_id: &str) -> Vec<String> {
    credentials
        .entries()
        .into_iter()
        .filter(|(key, _)| key.provider == provider_id)
        .map(|(key, _)| key.account)
        .collect()
}

fn accounts_for_provider(
    credentials: &CredentialStore,
    provider: &dyn goat_provider::ModelProvider,
) -> Option<Vec<String>> {
    let is_local = matches!(provider.capabilities().auth, AuthMethod::None);
    let stored = account_names_for(credentials, &provider.id().to_string());
    if is_local {
        if stored.is_empty() {
            return Some(vec![DEFAULT_ACCOUNT.to_owned()]);
        }
        return Some(stored);
    }
    if !stored.is_empty() {
        return Some(stored);
    }
    if provider.authenticated() {
        return Some(vec![DEFAULT_ACCOUNT.to_owned()]);
    }
    None
}

fn model_entry(
    provider_id: &str,
    model: &str,
    accounts: &[String],
    efforts: Vec<Effort>,
) -> ModelEntry {
    ModelEntry {
        provider: provider_id.to_owned(),
        model: model.to_owned(),
        context_window: None,
        efforts,
        accounts: accounts
            .iter()
            .map(|account| AccountChoice {
                id: account.clone(),
                display: account.clone(),
                target: ModelTarget {
                    provider: provider_id.to_owned(),
                    model: model.to_owned(),
                    account: account.clone(),
                    effort: None,
                },
            })
            .collect(),
    }
}

fn catalog_only(registry: &Registry, credentials: &CredentialStore) -> Vec<ModelEntry> {
    let mut entries = Vec::new();
    for provider in registry.all() {
        let Some(accounts) = accounts_for_provider(credentials, provider.as_ref()) else {
            continue;
        };
        let provider_id = provider.id().to_string();
        for &id in provider.catalog() {
            entries.push(model_entry(
                &provider_id,
                id,
                &accounts,
                provider.efforts(id),
            ));
        }
    }
    entries
}

async fn discover_ready(registry: &Registry, credentials: &CredentialStore) -> Vec<ModelEntry> {
    let mut entries = Vec::new();
    for provider in registry.all() {
        let Some(accounts) = accounts_for_provider(credentials, provider.as_ref()) else {
            continue;
        };
        let provider_id = provider.id().to_string();
        let (tx, mut rx) = mpsc::channel(32);
        let handle = provider.discover(tx);
        let mut discovered = Vec::new();
        let collect = async {
            while let Some(info) = rx.recv().await {
                discovered.push(info);
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(3), collect).await;
        handle.abort();

        let catalog = provider.catalog();
        let catalog_ids: HashSet<&str> = catalog.iter().copied().collect();

        for &id in catalog {
            entries.push(model_entry(
                &provider_id,
                id,
                &accounts,
                provider.efforts(id),
            ));
        }

        for info in discovered {
            if catalog_ids.contains(info.id.as_str()) {
                continue;
            }
            let efforts = provider.efforts(&info.id);
            entries.push(model_entry(&provider_id, &info.id, &accounts, efforts));
        }
    }
    entries
}

async fn run_round(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    provider: &dyn ModelProvider,
    request: ModelRequest,
    token: &CancellationToken,
) -> RoundResult {
    let (mev_tx, mut mev_rx) = mpsc::channel(64);
    let handle = provider.request(request, mev_tx);
    let mut raw = String::new();
    let mut thinking = String::new();
    let mut signature = String::new();
    let mut redacted: Vec<String> = Vec::new();
    let mut pending_calls: Vec<(String, String, String)> = Vec::new();
    let end = loop {
        tokio::select! {
            biased;
            () = token.cancelled() => {
                handle.abort();
                break RoundEnd::Cancelled;
            }
            maybe_event = mev_rx.recv() => match maybe_event {
                Some(ModelEvent::TextDelta { text }) => {
                    raw.push_str(&text);
                    let _ = ctx
                        .events
                        .send(Event::TextDelta { id: run.id, chunk: text })
                        .await;
                }
                Some(ModelEvent::ThinkingDelta { text }) => {
                    thinking.push_str(&text);
                    let _ = ctx
                        .events
                        .send(Event::ThinkingDelta { id: run.id, chunk: text })
                        .await;
                }
                Some(ModelEvent::ThinkingSignature { signature: sig }) => {
                    signature.push_str(&sig);
                }
                Some(ModelEvent::RedactedThinking { data }) => {
                    redacted.push(data);
                }
                Some(ModelEvent::ToolCall { id: vendor_id, name, input }) => {
                    pending_calls.push((vendor_id, name, input));
                }
                Some(ModelEvent::Completed) | None => break RoundEnd::Completed,
                Some(ModelEvent::Failed { message }) => break RoundEnd::Failed(message),
            }
        }
    };
    let thinking = (!thinking.is_empty()).then_some((thinking, signature));
    RoundResult {
        end,
        raw,
        thinking,
        redacted,
        pending_calls,
    }
}

fn tool_outcome(result: &Result<String, String>) -> (ToolOutcome, String) {
    match result {
        Ok(text) => (
            ToolOutcome {
                ok: true,
                summary: summarize_line(text),
            },
            text.clone(),
        ),
        Err(message) => (
            ToolOutcome {
                ok: false,
                summary: Some(message.clone()),
            },
            message.clone(),
        ),
    }
}

fn summarize_line(text: &str) -> Option<String> {
    text.lines().next().map(|line| {
        if line.chars().count() > 80 {
            let head: String = line.chars().take(80).collect();
            format!("{head}…")
        } else {
            line.to_owned()
        }
    })
}

async fn create_tool_call_record(
    ctx: &Ctx<'_>,
    ids: &TurnIds,
    vendor_id: &str,
    name: &str,
    input_json: &str,
) -> Option<i64> {
    let (Some(tid), Some(turn)) = (ids.stored_thread, ids.turn_db_id) else {
        return None;
    };
    match ctx
        .store
        .create_tool_call(NewToolCall {
            thread_id: tid,
            turn_id: turn,
            call_id: vendor_id.to_owned(),
            name: name.to_owned(),
            input: input_json.to_owned(),
            status: "running".to_owned(),
            started_at: now_ms(),
        })
        .await
    {
        Ok(id) => Some(id),
        Err(err) => {
            tracing::warn!(%err, "failed to create tool call record");
            None
        }
    }
}

async fn finish_tool_db(ctx: &Ctx<'_>, db_id: Option<i64>, outcome: &ToolOutcome) {
    let Some(db) = db_id else {
        return;
    };
    let status = if outcome.ok { "done" } else { "error" }.to_owned();
    if let Err(err) = ctx
        .store
        .finish_tool_call(db, status, outcome.summary.clone(), now_ms())
        .await
    {
        tracing::warn!(%err, "failed to finish tool call");
    }
}

async fn run_regular_tool(
    ctx: &Ctx<'_>,
    name: &str,
    input_json: &str,
    tool_ctx: &ToolContext,
    token: &CancellationToken,
) -> Option<Result<String, String>> {
    let fut = async {
        match ctx.tools.get(name) {
            Some(tool) => tool
                .run(input_json, tool_ctx)
                .await
                .map_err(|err| err.to_string()),
            None => Err(format!("unknown tool: {name}")),
        }
    };
    let mut fut = std::pin::pin!(fut);
    tokio::select! {
        biased;
        () = token.cancelled() => None,
        result = &mut fut => Some(result),
    }
}

const MAX_TOOL_RESULT_BYTES: usize = 64 * 1024;

fn cap_tool_result(mut content: String) -> String {
    if content.len() > MAX_TOOL_RESULT_BYTES {
        let boundary = content.floor_char_boundary(MAX_TOOL_RESULT_BYTES);
        content.truncate(boundary);
        content.push_str("\n[output truncated]\n");
    }
    content
}

async fn execute_tool(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    env: &LoopEnv<'_>,
    prep: &Prepared<'_>,
    tool_ctx: &ToolContext,
    token: &CancellationToken,
) -> ToolExecResult {
    let step = if prep.name == AGENT_TOOL_NAME && env.allow_delegate {
        match ctx.semaphore.acquire().await {
            Ok(_permit) if !token.is_cancelled() => {
                Some(run_delegation(ctx, env, prep.input_json, run.id, token).await)
            }
            _ => None,
        }
    } else {
        run_regular_tool(ctx, prep.name, prep.input_json, tool_ctx, token).await
    };
    let Some(result) = step else {
        let outcome = ToolOutcome {
            ok: false,
            summary: Some("interrupted".to_owned()),
        };
        finish_tool_db(ctx, prep.db_id, &outcome).await;
        let _ = ctx
            .events
            .send(Event::ToolDone {
                id: run.id,
                call: ToolCallId(prep.tui_id),
                outcome,
            })
            .await;
        return ToolExecResult {
            result_content: ContentBlock::ToolResult {
                tool_use_id: prep.vendor_id.to_owned(),
                content: "interrupted".to_owned(),
                is_error: true,
            },
            cancelled: true,
        };
    };
    let (outcome, result_text) = tool_outcome(&result);
    finish_tool_db(ctx, prep.db_id, &outcome).await;
    let is_error = !outcome.ok;
    let _ = ctx
        .events
        .send(Event::ToolDone {
            id: run.id,
            call: ToolCallId(prep.tui_id),
            outcome,
        })
        .await;
    ToolExecResult {
        result_content: ContentBlock::ToolResult {
            tool_use_id: prep.vendor_id.to_owned(),
            content: cap_tool_result(result_text),
            is_error,
        },
        cancelled: false,
    }
}

async fn run_tool_batch(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    env: &LoopEnv<'_>,
    pending_calls: &[(String, String, String)],
    call_seq: &mut u64,
    tool_ctx: &ToolContext,
    token: &CancellationToken,
) -> ToolBatchResult {
    let mut prepared: Vec<Prepared> = Vec::with_capacity(pending_calls.len());
    for (vendor_id, name, input_json) in pending_calls {
        *call_seq += 1;
        let tui_id = *call_seq;
        let _ = ctx
            .events
            .send(Event::ToolStarted {
                id: run.id,
                call: ToolCall {
                    id: ToolCallId(tui_id),
                    name: name.clone(),
                    input: input_json.clone(),
                },
            })
            .await;
        let db_id = match run.ids() {
            Some(ids) => create_tool_call_record(ctx, ids, vendor_id, name, input_json).await,
            None => None,
        };
        prepared.push(Prepared {
            vendor_id: vendor_id.as_str(),
            name: name.as_str(),
            input_json: input_json.as_str(),
            tui_id,
            db_id,
        });
    }
    let results = futures::future::join_all(
        prepared
            .iter()
            .map(|prep| execute_tool(ctx, run, env, prep, tool_ctx, token)),
    )
    .await;
    let mut tool_results = Vec::with_capacity(results.len());
    let mut cancelled = false;
    for result in results {
        if result.cancelled {
            cancelled = true;
        }
        tool_results.push(result.result_content);
    }
    ToolBatchResult {
        tool_results,
        cancelled,
    }
}

fn thread_title(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let title: String = trimmed.chars().take(60).collect();
    Some(title)
}

async fn ensure_thread(
    store: &Store,
    thread_id: &mut Option<i64>,
    target: &ModelTarget,
    title: Option<String>,
) -> Option<i64> {
    if let Some(tid) = thread_id {
        return Some(*tid);
    }
    let timestamp = now_ms();
    let cwd = std::env::current_dir()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    match store
        .create_thread(NewThread {
            cwd,
            title,
            provider: target.provider.clone(),
            model: target.model.clone(),
            account: target.account.clone(),
            effort: effort_string(target.effort),
            created_at: timestamp,
            updated_at: timestamp,
        })
        .await
    {
        Ok(id) => {
            *thread_id = Some(id);
            Some(id)
        }
        Err(err) => {
            tracing::warn!(%err, "failed to create thread");
            None
        }
    }
}

async fn init_db_turn(
    ctx: &Ctx<'_>,
    id: TaskId,
    text: &str,
    target: &ModelTarget,
    thread_id: &mut Option<i64>,
) -> TurnIds {
    let stored_thread = ensure_thread(ctx.store, thread_id, target, thread_title(text)).await;
    let turn_db_id = if let Some(tid) = stored_thread {
        let body = serde_json::to_string(&vec![ContentBlock::Text {
            text: text.to_owned(),
        }])
        .unwrap_or_else(|_| text.to_owned());
        if let Err(err) = ctx
            .store
            .create_message(NewMessage {
                thread_id: tid,
                turn_id: None,
                role: "user".to_owned(),
                body,
                created_at: now_ms(),
            })
            .await
        {
            tracing::warn!(%err, "failed to persist user message");
        }
        ctx.store
            .create_turn(NewTurn {
                thread_id: tid,
                task_id: i64::try_from(id.0).unwrap_or(i64::MAX),
                provider: target.provider.clone(),
                model: target.model.clone(),
                account: target.account.clone(),
                effort: effort_string(target.effort),
                status: "running".to_owned(),
                started_at: now_ms(),
            })
            .await
            .ok()
    } else {
        None
    };
    TurnIds {
        stored_thread,
        turn_db_id,
    }
}

async fn persist_message(ctx: &Ctx<'_>, ids: &TurnIds, message: &ProviderMessage) {
    let role = match message.role {
        MessageRole::System => return,
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    };
    let Some(tid) = ids.stored_thread else {
        return;
    };
    let Ok(body) = serde_json::to_string(&message.content) else {
        return;
    };
    if let Err(err) = ctx
        .store
        .create_message(NewMessage {
            thread_id: tid,
            turn_id: ids.turn_db_id,
            role: role.to_owned(),
            body,
            created_at: now_ms(),
        })
        .await
    {
        tracing::warn!(%err, "failed to persist message");
    }
}

async fn finalize_turn(ctx: &Ctx<'_>, id: TaskId, outcome: &TurnEnd, ids: &TurnIds) {
    match outcome {
        TurnEnd::Done => {
            if let Some(turn) = ids.turn_db_id
                && let Err(err) = ctx
                    .store
                    .finish_turn(turn, "done".to_owned(), now_ms())
                    .await
            {
                tracing::warn!(%err, "failed to finish turn");
            }
            let _ = ctx
                .events
                .send(Event::TaskDone {
                    id,
                    interrupted: false,
                })
                .await;
        }
        TurnEnd::Interrupted => {
            if let Some(turn) = ids.turn_db_id
                && let Err(err) = ctx
                    .store
                    .finish_turn(turn, "interrupted".to_owned(), now_ms())
                    .await
            {
                tracing::warn!(%err, "failed to finish turn");
            }
            let _ = ctx
                .events
                .send(Event::TaskDone {
                    id,
                    interrupted: true,
                })
                .await;
        }
        TurnEnd::Failed(message) => {
            let _ = ctx
                .events
                .send(Event::Error {
                    id: Some(id),
                    message: message.clone(),
                })
                .await;
            if let Some(turn) = ids.turn_db_id
                && let Err(err) = ctx
                    .store
                    .finish_turn(turn, "error".to_owned(), now_ms())
                    .await
            {
                tracing::warn!(%err, "failed to finish turn");
            }
            let _ = ctx
                .events
                .send(Event::TaskDone {
                    id,
                    interrupted: true,
                })
                .await;
        }
        TurnEnd::Shutdown => {}
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_round_output(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    env: &LoopEnv<'_>,
    round: RoundResult,
    history: &mut Vec<ProviderMessage>,
    rounds: usize,
    call_seq: &mut u64,
    tool_ctx: &ToolContext,
    token: &CancellationToken,
) -> RoundOutcome {
    let raw = round.raw;
    let pending_calls = round.pending_calls;
    let shown_text = (!raw.is_empty()).then(|| raw.clone());
    if !raw.is_empty()
        || !pending_calls.is_empty()
        || round.thinking.is_some()
        || !round.redacted.is_empty()
    {
        let mut content = Vec::new();
        if let Some((text, signature)) = &round.thinking {
            content.push(ContentBlock::Thinking {
                text: text.clone(),
                signature: signature.clone(),
            });
        }
        for data in &round.redacted {
            content.push(ContentBlock::RedactedThinking { data: data.clone() });
        }
        if !raw.is_empty() {
            content.push(ContentBlock::Text { text: raw.clone() });
        }
        for (vendor_id, name, input_json) in &pending_calls {
            let input_val = serde_json::from_str(input_json).unwrap_or(serde_json::Value::Null);
            content.push(ContentBlock::ToolUse {
                id: vendor_id.clone(),
                name: name.clone(),
                input: input_val,
            });
        }
        let message = ProviderMessage {
            role: MessageRole::Assistant,
            content,
        };
        if let Some(ids) = run.ids() {
            persist_message(ctx, ids, &message).await;
        }
        history.push(message);
    }
    if pending_calls.is_empty() {
        if let Some(shown) = shown_text {
            let _ = ctx
                .events
                .send(Event::TextDone {
                    id: run.id,
                    text: shown,
                })
                .await;
        }
        return RoundOutcome::Done;
    }
    if rounds >= MAX_TOOL_ROUNDS {
        tracing::warn!(rounds, "tool round cap reached; ending run");
        let synthetic: Vec<ContentBlock> = pending_calls
            .iter()
            .map(|(vendor_id, _, _)| ContentBlock::ToolResult {
                tool_use_id: vendor_id.clone(),
                content: "tool round limit reached".to_owned(),
                is_error: true,
            })
            .collect();
        let message = ProviderMessage {
            role: MessageRole::User,
            content: synthetic,
        };
        if let Some(ids) = run.ids() {
            persist_message(ctx, ids, &message).await;
        }
        history.push(message);
        if let Some(shown) = shown_text {
            let _ = ctx
                .events
                .send(Event::TextDone {
                    id: run.id,
                    text: shown,
                })
                .await;
        }
        return RoundOutcome::Done;
    }
    let batch = run_tool_batch(ctx, run, env, &pending_calls, call_seq, tool_ctx, token).await;
    let message = ProviderMessage {
        role: MessageRole::User,
        content: batch.tool_results,
    };
    if let Some(ids) = run.ids() {
        persist_message(ctx, ids, &message).await;
    }
    history.push(message);
    if batch.cancelled {
        RoundOutcome::Cancelled
    } else {
        RoundOutcome::Continue
    }
}

fn build_tool_defs(
    ctx: &Ctx<'_>,
    provider: &dyn ModelProvider,
    selection: Option<&ToolSelection>,
    allow_delegate: bool,
) -> Vec<ToolDefinition> {
    if !provider.capabilities().tools {
        return Vec::new();
    }
    let mut defs: Vec<ToolDefinition> = ctx
        .tools
        .specs()
        .into_iter()
        .filter(|spec| selection.is_none_or(|sel| sel.allows(spec.name)))
        .map(|spec| ToolDefinition {
            name: spec.name.to_owned(),
            description: spec.description.to_owned(),
            input_schema: spec.parameters,
        })
        .collect();
    if allow_delegate && !ctx.agents.is_empty() {
        defs.push(agent_tool_def(ctx));
    }
    defs
}

fn agent_tool_def(ctx: &Ctx<'_>) -> ToolDefinition {
    let names: Vec<String> = ctx.agents.names();
    let mut description = String::from(
        "Delegate a self-contained task to a sub-agent that runs in its own context with a restricted tool set and returns only its final report. Prefer this for focused investigation or work that would otherwise flood the main context. Issue several Agent calls in one response to run them in parallel. Available agent_type values:",
    );
    for spec in ctx.agents.iter() {
        let _ = write!(description, "\n- {}: {}", spec.name, spec.description);
    }
    ToolDefinition {
        name: AGENT_TOOL_NAME.to_owned(),
        description,
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "agent_type": {
                    "type": "string",
                    "enum": names,
                },
                "prompt": {
                    "type": "string",
                    "description": "A complete, self-contained instruction for the sub-agent. It does not see the conversation, so include all needed context.",
                },
            },
            "required": ["agent_type", "prompt"],
        }),
    }
}

async fn core_loop(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    env: &LoopEnv<'_>,
    token: &CancellationToken,
    history: &mut Vec<ProviderMessage>,
) -> LoopOutcome {
    let tool_ctx = match ToolContext::new(env.cwd) {
        Ok(tool_ctx) => tool_ctx,
        Err(err) => return LoopOutcome::Failed(err.to_string()),
    };
    let mut rounds = 0usize;
    let mut call_seq = 0u64;
    loop {
        rounds += 1;
        let request = ModelRequest {
            model: env.target.model.clone(),
            messages: history.clone(),
            tools: env.tool_defs.to_vec(),
            effort: env.target.effort,
        };
        let round = run_round(ctx, run, env.provider, request, token).await;
        match round.end {
            RoundEnd::Cancelled => return LoopOutcome::Cancelled,
            RoundEnd::Failed(message) => return LoopOutcome::Failed(message),
            RoundEnd::Completed => {}
        }
        match process_round_output(
            ctx,
            run,
            env,
            round,
            history,
            rounds,
            &mut call_seq,
            &tool_ctx,
            token,
        )
        .await
        {
            RoundOutcome::Done => return LoopOutcome::Completed,
            RoundOutcome::Cancelled => return LoopOutcome::Cancelled,
            RoundOutcome::Continue => {}
        }
    }
}

fn resolve_agent_model(
    ctx: &Ctx<'_>,
    parent: &ModelTarget,
    spec: &AgentSpec,
) -> Option<(Arc<dyn ModelProvider>, String, Option<Effort>)> {
    if let Some(model_id) = &spec.model {
        if let Some(found) = ctx
            .registry
            .all()
            .iter()
            .find(|provider| provider.catalog().contains(&model_id.as_str()))
        {
            let provider_id = found.id().to_string();
            let provider =
                goat_providers::build_provider(ctx.credentials, &provider_id, &parent.account)
                    .unwrap_or_else(|| found.clone());
            let effort = spec
                .effort
                .or_else(|| provider.efforts(model_id).into_iter().next());
            return Some((provider, model_id.clone(), effort));
        }
        tracing::warn!(model = %model_id, "agent model not found; inheriting parent model");
    }
    let provider =
        goat_providers::build_provider(ctx.credentials, &parent.provider, &parent.account)
            .or_else(|| {
                ctx.registry
                    .get(&goat_provider::ProviderId::from(parent.provider.as_str()))
            })?;
    Some((provider, parent.model.clone(), parent.effort))
}

async fn run_delegation(
    ctx: &Ctx<'_>,
    env: &LoopEnv<'_>,
    input_json: &str,
    parent: TaskId,
    token: &CancellationToken,
) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Input {
        agent_type: String,
        prompt: String,
    }
    let args: Input =
        serde_json::from_str(input_json).map_err(|err| format!("invalid Agent input: {err}"))?;
    let Some(spec) = ctx.agents.get(&args.agent_type) else {
        return Err(format!("unknown agent_type: {}", args.agent_type));
    };
    let Some((provider, model, effort)) = resolve_agent_model(ctx, env.target, spec) else {
        return Err("could not resolve a model for the agent".to_owned());
    };
    let child_target = ModelTarget {
        provider: provider.id().to_string(),
        model,
        account: env.target.account.clone(),
        effort,
    };
    let tool_defs = build_tool_defs(ctx, provider.as_ref(), Some(&spec.tools), false);
    let mut history = vec![
        ProviderMessage::text(MessageRole::System, spec.prompt.clone()),
        ProviderMessage::text(MessageRole::User, args.prompt.clone()),
    ];
    let child_id = TaskId(ctx.child_ids.fetch_add(1, Ordering::Relaxed));
    let _ = ctx
        .events
        .send(Event::AgentStarted {
            id: child_id,
            parent,
            agent_type: args.agent_type.clone(),
            label: delegation_label(&args.prompt),
        })
        .await;
    let run = Run::child(child_id);
    let child_env = LoopEnv {
        provider: provider.as_ref(),
        target: &child_target,
        tool_defs: &tool_defs,
        cwd: env.cwd,
        allow_delegate: false,
    };
    let child_token = token.child_token();
    let outcome = Box::pin(core_loop(ctx, &run, &child_env, &child_token, &mut history)).await;
    let result = match outcome {
        LoopOutcome::Completed => Ok(final_text(&history)),
        LoopOutcome::Cancelled => Ok("(agent interrupted)".to_owned()),
        LoopOutcome::Failed(message) => Err(message),
    };
    let _ = ctx
        .events
        .send(Event::AgentDone {
            id: child_id,
            ok: result.is_ok(),
        })
        .await;
    result
}

fn delegation_label(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or("").trim();
    if line.chars().count() > 50 {
        let head: String = line.chars().take(50).collect();
        format!("{head}…")
    } else {
        line.to_owned()
    }
}

fn final_text(history: &[ProviderMessage]) -> String {
    for message in history.iter().rev() {
        if message.role == MessageRole::Assistant {
            let mut text = String::new();
            for block in &message.content {
                if let ContentBlock::Text { text: chunk } = block {
                    text.push_str(chunk);
                }
            }
            if !text.trim().is_empty() {
                return text;
            }
        }
    }
    "(agent produced no output)".to_owned()
}

async fn resolve_thread_cwd(ctx: &Ctx<'_>, stored_thread: Option<i64>) -> std::path::PathBuf {
    match stored_thread {
        Some(tid) => ctx
            .store
            .get_thread(tid)
            .await
            .ok()
            .flatten()
            .map(|thread| thread.cwd)
            .filter(|cwd| !cwd.is_empty())
            .map_or_else(
                || std::env::current_dir().unwrap_or_default(),
                std::path::PathBuf::from,
            ),
        None => std::env::current_dir().unwrap_or_default(),
    }
}

async fn emit_task_error(ctx: &Ctx<'_>, id: TaskId, message: String) {
    let _ = ctx
        .events
        .send(Event::Error {
            id: Some(id),
            message,
        })
        .await;
    let _ = ctx
        .events
        .send(Event::TaskDone {
            id,
            interrupted: true,
        })
        .await;
}

async fn handle_idle_op(
    op: Op,
    store: &Store,
    thread_id: Option<i64>,
    target: &mut Option<ModelTarget>,
    events: &mpsc::Sender<Event>,
) {
    if matches!(
        op,
        Op::AddAccount { .. } | Op::RemoveAccount { .. } | Op::SetTheme { .. }
    ) {
        return;
    }
    if let Op::SelectModel { target: chosen } = op {
        if let Some(tid) = thread_id
            && let Err(err) = store
                .update_thread_model(
                    tid,
                    chosen.provider.clone(),
                    chosen.model.clone(),
                    chosen.account.clone(),
                    effort_string(chosen.effort),
                    now_ms(),
                )
                .await
        {
            tracing::warn!(%err, "failed to update thread model");
        }
        *target = Some(chosen.clone());
        let _ = events.send(Event::ModelSelected { target: chosen }).await;
    }
}

async fn handle_list_threads(store: &Store, events: &mpsc::Sender<Event>) {
    let cwd = std::env::current_dir()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let threads = store.list_threads_in(cwd, 50).await.unwrap_or_default();
    let summaries = threads
        .into_iter()
        .map(|thread| ThreadSummary {
            model: format!("{}/{}", thread.provider, thread.model),
            title: thread
                .title
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| format!("{}/{}", thread.provider, thread.model)),
            id: thread.id,
            updated_at: thread.updated_at,
        })
        .collect();
    let _ = events
        .send(Event::ThreadsListed { threads: summaries })
        .await;
}

fn tool_summary(content: &str) -> String {
    content
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn handle_resume(
    store: &Store,
    skills: &[SkillInfo],
    tid: i64,
    target: &mut Option<ModelTarget>,
    history: &mut Vec<ProviderMessage>,
    thread_id: &mut Option<i64>,
    events: &mpsc::Sender<Event>,
) {
    let Some(thread) = store.get_thread(tid).await.ok().flatten() else {
        return;
    };
    let new_target = ModelTarget {
        provider: thread.provider.clone(),
        model: thread.model.clone(),
        account: thread.account.clone(),
        effort: thread.effort.as_deref().and_then(Effort::parse),
    };
    let messages = store.get_messages(tid).await.unwrap_or_default();
    let mut new_history = vec![ProviderMessage::text(
        MessageRole::System,
        build_system_prompt(skills),
    )];
    let mut entries: Vec<TranscriptEntry> = Vec::new();
    let mut tool_uses: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    let mut tool_seq: u64 = 0;
    for stored in messages {
        let role = match stored.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            _ => continue,
        };
        let content = parse_content_blocks(&stored.body);
        for block in &content {
            match block {
                ContentBlock::Text { text } => match role {
                    MessageRole::User => entries.push(TranscriptEntry::User(text.clone())),
                    MessageRole::Assistant => {
                        entries.push(TranscriptEntry::Assistant(text.clone()));
                    }
                    MessageRole::System => {}
                },
                ContentBlock::ToolUse { id, name, input } => {
                    tool_uses.insert(id.clone(), (name.clone(), input.to_string()));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    if let Some((name, input)) = tool_uses.remove(tool_use_id) {
                        tool_seq += 1;
                        entries.push(TranscriptEntry::Tool {
                            call: ToolCall {
                                id: ToolCallId(tool_seq),
                                name,
                                input,
                            },
                            outcome: ToolOutcome {
                                ok: !is_error,
                                summary: Some(tool_summary(content)),
                            },
                        });
                    }
                }
                ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {}
            }
        }
        new_history.push(ProviderMessage { role, content });
    }
    *history = new_history;
    *thread_id = Some(tid);
    *target = Some(new_target.clone());
    let _ = events
        .send(Event::ConversationRestored {
            target: new_target,
            entries,
        })
        .await;
}

async fn handle_turn(
    ctx: &Ctx<'_>,
    id: TaskId,
    text: String,
    target: &mut Option<ModelTarget>,
    history: &mut Vec<ProviderMessage>,
    thread_id: &mut Option<i64>,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    let Some(resolved) = target.clone() else {
        emit_task_error(ctx, id, "no model selected".to_owned()).await;
        return Flow::Continue;
    };
    let resolved_provider =
        goat_providers::build_provider(ctx.credentials, &resolved.provider, &resolved.account)
            .or_else(|| {
                ctx.registry
                    .get(&goat_provider::ProviderId::from(resolved.provider.as_str()))
            });
    let Some(provider) = resolved_provider else {
        emit_task_error(ctx, id, format!("unknown provider: {}", resolved.provider)).await;
        return Flow::Continue;
    };

    if history.is_empty() {
        history.push(ProviderMessage::text(
            MessageRole::System,
            build_system_prompt(ctx.skills),
        ));
    }
    history.push(ProviderMessage::text(MessageRole::User, text.clone()));
    let ids = init_db_turn(ctx, id, &text, &resolved, thread_id).await;
    if ctx.events.send(Event::TaskStarted { id }).await.is_err() {
        finalize_turn(ctx, id, &TurnEnd::Shutdown, &ids).await;
        return Flow::Shutdown;
    }

    let cwd = resolve_thread_cwd(ctx, ids.stored_thread).await;
    let tool_defs = build_tool_defs(ctx, provider.as_ref(), None, true);
    let run = Run::top(id, &ids);
    let env = LoopEnv {
        provider: provider.as_ref(),
        target: &resolved,
        tool_defs: &tool_defs,
        cwd: &cwd,
        allow_delegate: true,
    };
    let token = CancellationToken::new();
    let mut shutdown = false;
    let mut deferred: Vec<Op> = Vec::new();

    let outcome = {
        let core = core_loop(ctx, &run, &env, &token, history);
        tokio::pin!(core);
        loop {
            tokio::select! {
                biased;
                result = &mut core => break result,
                maybe_op = ops.recv() => match maybe_op {
                    Some(Op::Interrupt { id: target_id }) if target_id == id => token.cancel(),
                    Some(Op::Shutdown) | None => {
                        shutdown = true;
                        token.cancel();
                    }
                    Some(op) => deferred.push(op),
                },
            }
        }
    };

    let turn_end = match outcome {
        LoopOutcome::Completed => TurnEnd::Done,
        LoopOutcome::Failed(message) => TurnEnd::Failed(message),
        LoopOutcome::Cancelled => {
            if shutdown {
                TurnEnd::Shutdown
            } else {
                TurnEnd::Interrupted
            }
        }
    };
    finalize_turn(ctx, id, &turn_end, &ids).await;
    if matches!(turn_end, TurnEnd::Shutdown) {
        return Flow::Shutdown;
    }

    for op in deferred {
        handle_idle_op(op, ctx.store, *thread_id, target, ctx.events).await;
    }

    Flow::Continue
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use goat_auth::CredentialStore;
    use goat_core::Session;
    use goat_protocol::{Event, ModelTarget, Op, TaskId};
    use goat_provider::{
        AuthMethod, ModelEvent, ModelInfo, ModelProvider, ModelRequest, ProviderCapabilities,
        ProviderId,
    };
    use goat_providers::Registry;
    use goat_store::Store;
    use tokio::{sync::mpsc, task::JoinHandle};

    use super::GoatAgent;

    struct MockProvider {
        id: String,
        reply: String,
        delay_ms: u64,
    }

    impl ModelProvider for MockProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from(self.id.as_str())
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                tools: false,
                auth: AuthMethod::None,
            }
        }

        fn request(&self, _req: ModelRequest, events: mpsc::Sender<ModelEvent>) -> JoinHandle<()> {
            let reply = self.reply.clone();
            let delay = self.delay_ms;
            tokio::spawn(async move {
                if delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
                let _ = events.send(ModelEvent::TextDelta { text: reply }).await;
                let _ = events.send(ModelEvent::Completed).await;
            })
        }

        fn discover(&self, out: mpsc::Sender<ModelInfo>) -> JoinHandle<()> {
            tokio::spawn(async move {
                drop(out);
            })
        }
    }

    struct ScriptedProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl ModelProvider for ScriptedProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                tools: true,
                auth: AuthMethod::None,
            }
        }

        fn request(&self, _req: ModelRequest, events: mpsc::Sender<ModelEvent>) -> JoinHandle<()> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::spawn(async move {
                match n {
                    0 => {
                        let _ = events
                            .send(ModelEvent::ToolCall {
                                id: "call-1".to_owned(),
                                name: "Agent".to_owned(),
                                input: "{\"agent_type\":\"explore\",\"prompt\":\"look into it\"}"
                                    .to_owned(),
                            })
                            .await;
                    }
                    1 => {
                        let _ = events
                            .send(ModelEvent::TextDelta {
                                text: "child findings".to_owned(),
                            })
                            .await;
                    }
                    _ => {
                        let _ = events
                            .send(ModelEvent::TextDelta {
                                text: "final answer".to_owned(),
                            })
                            .await;
                    }
                }
                let _ = events.send(ModelEvent::Completed).await;
            })
        }

        fn discover(&self, out: mpsc::Sender<ModelInfo>) -> JoinHandle<()> {
            tokio::spawn(async move {
                drop(out);
            })
        }
    }

    #[tokio::test]
    async fn delegates_to_agent_and_returns_result() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = ScriptedProvider {
            calls: calls.clone(),
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().unwrap();
        let credentials =
            CredentialStore::new(std::env::temp_dir().join("goat-agent-delegate.json"));
        let agent = GoatAgent::new(registry, store, credentials, Some(target("mock")));
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "do it".to_owned(),
        })
        .await
        .unwrap();

        let mut agent_started = false;
        let mut agent_done_ok = false;
        let mut final_text = String::new();
        while let Some(event) = events.recv().await {
            match event {
                Event::ToolStarted { call, .. } if call.name == "Agent" => agent_started = true,
                Event::ToolDone { outcome, .. } => agent_done_ok = outcome.ok,
                Event::TextDone { text, .. } => final_text = text,
                Event::TaskDone { interrupted, .. } => {
                    assert!(!interrupted);
                    break;
                }
                _ => {}
            }
        }
        assert!(agent_started, "expected the Agent tool to start");
        assert!(agent_done_ok, "expected the Agent delegation to succeed");
        assert_eq!(final_text, "final answer");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    fn target(provider: &str) -> ModelTarget {
        ModelTarget {
            provider: provider.to_owned(),
            model: "m".to_owned(),
            account: "default".to_owned(),
            effort: None,
        }
    }

    fn agent_with(reply: &str, delay_ms: u64) -> GoatAgent {
        let provider = MockProvider {
            id: "mock".to_owned(),
            reply: reply.to_owned(),
            delay_ms,
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().unwrap();
        let credentials = CredentialStore::new(std::env::temp_dir().join("goat-agent-test.json"));
        GoatAgent::new(registry, store, credentials, Some(target("mock")))
    }

    #[tokio::test]
    async fn bridges_text_to_protocol_events() {
        let session = Session::spawn(agent_with("hello", 0));
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "hi".to_owned(),
        })
        .await
        .unwrap();

        let mut started = false;
        let mut deltas = String::new();
        let mut done = false;
        while let Some(event) = events.recv().await {
            match event {
                Event::ModelListChanged { .. }
                | Event::ModelSelected { .. }
                | Event::LoginProviders { .. }
                | Event::LoginStatus { .. }
                | Event::AccountsChanged { .. }
                | Event::SkillsChanged { .. }
                | Event::TextDone { .. } => {}
                Event::TaskStarted { .. } => started = true,
                Event::TextDelta { chunk, .. } => deltas.push_str(&chunk),
                Event::TaskDone { interrupted, .. } => {
                    assert!(!interrupted);
                    done = true;
                    break;
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert!(started);
        assert_eq!(deltas, "hello");
        assert!(done);
    }

    #[test]
    fn system_prompt_without_skills_is_base() {
        assert_eq!(super::build_system_prompt(&[]), super::SYSTEM_PROMPT);
    }

    #[test]
    fn system_prompt_lists_skills() {
        let prompt = super::build_system_prompt(&[goat_protocol::SkillInfo {
            name: "demo".to_owned(),
            description: "does the demo".to_owned(),
        }]);
        assert!(prompt.contains("demo"));
        assert!(prompt.contains("does the demo"));
        assert!(prompt.contains("Skill tool"));
    }

    #[tokio::test]
    async fn interrupt_ends_turn() {
        let session = Session::spawn(agent_with("late", 5_000));
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(9),
            text: "hi".to_owned(),
        })
        .await
        .unwrap();
        ops.send(Op::Interrupt { id: TaskId(9) }).await.unwrap();

        let mut interrupted = false;
        while let Some(event) = events.recv().await {
            match event {
                Event::TextDone { .. } => panic!("interrupted turn must not finalize text"),
                Event::TaskDone {
                    interrupted: was, ..
                } => {
                    interrupted = was;
                    break;
                }
                _ => {}
            }
        }
        assert!(interrupted);
    }

    #[tokio::test]
    async fn unknown_provider_errors() {
        let registry = Registry::from_providers(vec![]);
        let store = Store::open_in_memory().unwrap();
        let credentials = CredentialStore::new(std::env::temp_dir().join("goat-agent-ghost.json"));
        let agent = GoatAgent::new(registry, store, credentials, Some(target("ghost")));
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();
        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "hi".to_owned(),
        })
        .await
        .unwrap();

        let mut saw_error = false;
        while let Some(event) = events.recv().await {
            match event {
                Event::Error { .. } => saw_error = true,
                Event::TaskDone { .. } => break,
                _ => {}
            }
        }
        assert!(saw_error);
    }

    async fn drain_until_task_done(events: &mut mpsc::Receiver<Event>) {
        while let Some(event) = events.recv().await {
            if matches!(event, Event::TaskDone { .. }) {
                return;
            }
        }
    }

    #[tokio::test]
    async fn clear_starts_new_thread() {
        let provider = MockProvider {
            id: "mock".to_owned(),
            reply: "ok".to_owned(),
            delay_ms: 0,
        };
        let registry = Registry::from_providers(vec![Arc::new(provider)]);
        let store = Store::open_in_memory().unwrap();
        let credentials = CredentialStore::new(std::env::temp_dir().join("goat-agent-clear.json"));
        let agent = GoatAgent::new(registry, store.clone(), credentials, Some(target("mock")));
        let session = Session::spawn(agent);
        let (ops, mut events, _handle) = session.into_parts();

        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "first".to_owned(),
        })
        .await
        .unwrap();
        drain_until_task_done(&mut events).await;

        ops.send(Op::Clear).await.unwrap();

        ops.send(Op::SubmitMessage {
            id: TaskId(2),
            text: "second".to_owned(),
        })
        .await
        .unwrap();
        drain_until_task_done(&mut events).await;

        assert!(store.get_thread(2).await.unwrap().is_some());
    }
}
