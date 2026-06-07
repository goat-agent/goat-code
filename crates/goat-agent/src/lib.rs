use std::{
    collections::HashSet,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use goat_auth::{
    CredentialKey, CredentialKind, CredentialStore, RedactionSet, ResolvedCredential, SecretString,
};
use goat_core::Engine;
use goat_protocol::{
    AccountChoice, AccountEntry, AccountInfo, AuthMethod, Event, LoginCredential, LoginProvider,
    ModelEntry, ModelTarget, NotifyKind, Op, TaskId, ToolCallId,
};
use goat_provider::{
    ContentBlock, MessageRole, ModelEvent, ModelRequest, ProviderMessage, ToolDefinition,
};
use goat_providers::{DEFAULT_ACCOUNT, Registry};
use goat_store::{NewMessage, NewThread, NewTurn, Store};
use goat_tool::{ToolContext, outcome_from};
use goat_tools::ToolRegistry;
use tokio::{sync::mpsc, task::JoinHandle};

const MAX_TOOL_ROUNDS: usize = 20;
const SYSTEM_PROMPT: &str = "You are Goat, an expert software engineering assistant. You help users understand, build, and improve software by reading code, running tools, and providing accurate, actionable guidance. When using tools, prefer targeted reads and searches over broad exploration. Always verify your understanding before making changes.";

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
    store: &'a Store,
    redaction: &'a RedactionSet,
    events: &'a mpsc::Sender<Event>,
}

enum Flow {
    Continue,
    Shutdown,
}

enum RoundEnd {
    Completed,
    Interrupted,
    Failed(String),
    ShuttingDown,
}

struct RoundResult {
    end: RoundEnd,
    raw: String,
    pending_calls: Vec<(String, String, String)>,
}

enum ToolResult {
    Done(Result<String, goat_tool::ToolError>),
    Interrupted,
    ShuttingDown,
}

struct ToolExecResult {
    result_content: ContentBlock,
    interrupted: bool,
    shutting_down: bool,
}

struct ToolBatchResult {
    tool_results: Vec<ContentBlock>,
    interrupted_at: Option<usize>,
    shutting_down: bool,
}

struct ToolCallSpec<'a> {
    vendor_id: &'a str,
    name: &'a str,
    input_json: &'a str,
    tui_id: u64,
    db_id: Option<i64>,
}

struct TurnIds {
    stored_thread: Option<i64>,
    turn_db_id: Option<i64>,
}

enum RoundOutcome {
    Done(RoundEnd),
    Continue,
    Shutdown,
}

struct RoundState<'a> {
    ids: &'a TurnIds,
    rounds: usize,
    cwd: &'a std::path::Path,
    call_seq: &'a mut u64,
    deferred: &'a mut Vec<Op>,
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

async fn run(agent: GoatAgent, mut ops: mpsc::Receiver<Op>, events: mpsc::Sender<Event>) {
    let GoatAgent {
        mut registry,
        tools,
        store,
        credentials,
        mut target,
    } = agent;
    let mut redaction = RedactionSet::new();
    let mut history: Vec<ProviderMessage> = Vec::new();
    let mut thread_id: Option<i64> = None;

    if target.is_none() {
        target = restore_target(&store, &credentials).await;
    }
    announce_startup(&events, &registry, &credentials, target.as_ref()).await;

    while let Some(op) = ops.recv().await {
        match op {
            Op::SubmitMessage { id, text } => {
                let ctx = Ctx {
                    registry: &registry,
                    credentials: &credentials,
                    tools: &tools,
                    store: &store,
                    redaction: &redaction,
                    events: &events,
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
                    redaction: &mut redaction,
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
                    redaction: &mut redaction,
                    events: &events,
                };
                handle_login(ctx, provider, name, credential, true).await;
            }
            Op::RemoveAccount { provider, name } => {
                handle_remove_account(provider, name, &credentials, &mut registry, &events).await;
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
    redaction: &'a mut RedactionSet,
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
            ctx.redaction.insert_credential(&resolved);
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
                    ctx.redaction.insert_credential(&resolved);
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

fn model_entry(provider_id: &str, model: &str, accounts: &[String]) -> ModelEntry {
    ModelEntry {
        provider: provider_id.to_owned(),
        model: model.to_owned(),
        context_window: None,
        accounts: accounts
            .iter()
            .map(|account| AccountChoice {
                id: account.clone(),
                display: account.clone(),
                target: ModelTarget {
                    provider: provider_id.to_owned(),
                    model: model.to_owned(),
                    account: account.clone(),
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
            entries.push(model_entry(&provider_id, id, &accounts));
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
            entries.push(model_entry(&provider_id, id, &accounts));
        }

        for info in discovered {
            if catalog_ids.contains(info.id.as_str()) {
                continue;
            }
            entries.push(model_entry(&provider_id, &info.id, &accounts));
        }
    }
    entries
}

async fn run_round(
    id: TaskId,
    provider: &dyn goat_provider::ModelProvider,
    request: ModelRequest,
    ops: &mut mpsc::Receiver<Op>,
    events: &mpsc::Sender<Event>,
    redaction: &RedactionSet,
    deferred: &mut Vec<Op>,
) -> RoundResult {
    let (mev_tx, mut mev_rx) = mpsc::channel(64);
    let handle = provider.request(request, mev_tx);
    let mut raw = String::new();
    let mut pending_calls: Vec<(String, String, String)> = Vec::new();
    let mut pending_tail = String::new();
    let window = redaction.max_secret_len().max(64);
    let end = loop {
        tokio::select! {
            biased;
            maybe_op = ops.recv() => match maybe_op {
                Some(Op::Interrupt { .. }) => { handle.abort(); break RoundEnd::Interrupted; }
                Some(Op::Shutdown) | None => {
                    handle.abort();
                    break RoundEnd::ShuttingDown;
                }
                Some(op) => { deferred.push(op); }
            },
            maybe_event = mev_rx.recv() => match maybe_event {
                Some(ModelEvent::TextDelta { text }) => {
                    raw.push_str(&text);
                    pending_tail.push_str(&text);
                    let safe_len = pending_tail.len().saturating_sub(window);
                    let safe_boundary = pending_tail.floor_char_boundary(safe_len);
                    let safe = pending_tail[..safe_boundary].to_owned();
                    pending_tail = pending_tail[safe_boundary..].to_owned();
                    if !safe.is_empty() {
                        let shown = redaction.redact(&safe);
                        if events.send(Event::TextDelta { id, chunk: shown }).await.is_err() {
                            handle.abort();
                            break RoundEnd::ShuttingDown;
                        }
                    }
                }
                Some(ModelEvent::ToolCall { id: vendor_id, name, input }) => {
                    pending_calls.push((vendor_id, name, input));
                }
                Some(ModelEvent::Completed) | None => break RoundEnd::Completed,
                Some(ModelEvent::Failed { message }) => break RoundEnd::Failed(message),
            }
        }
    };
    if !pending_tail.is_empty() {
        let shown = redaction.redact(&pending_tail);
        let _ = events.send(Event::TextDelta { id, chunk: shown }).await;
    }
    RoundResult {
        end,
        raw,
        pending_calls,
    }
}

async fn finish_tool_interrupted(
    ctx: &Ctx<'_>,
    id: TaskId,
    vendor_id: &str,
    tui_id: u64,
    db_id: Option<i64>,
) -> ContentBlock {
    if let Some(db) = db_id
        && let Err(err) = ctx
            .store
            .finish_tool_call(
                db,
                "interrupted".to_owned(),
                Some("interrupted".to_owned()),
                now_ms(),
            )
            .await
    {
        tracing::warn!(%err, "failed to finish tool call");
    }
    let _ = ctx
        .events
        .send(Event::ToolDone {
            id,
            call: ToolCallId(tui_id),
            outcome: goat_protocol::ToolOutcome {
                ok: false,
                summary: Some("interrupted".to_owned()),
            },
        })
        .await;
    ContentBlock::ToolResult {
        tool_use_id: vendor_id.to_owned(),
        content: "interrupted".to_owned(),
        is_error: true,
    }
}

async fn finish_tool_success(
    ctx: &Ctx<'_>,
    id: TaskId,
    vendor_id: &str,
    tui_id: u64,
    db_id: Option<i64>,
    result: &Result<String, goat_tool::ToolError>,
) -> ContentBlock {
    let (outcome, result_text) = outcome_from(result);
    if let Some(db) = db_id {
        let status = if outcome.ok { "done" } else { "error" }.to_owned();
        if let Err(err) = ctx
            .store
            .finish_tool_call(db, status, outcome.summary.clone(), now_ms())
            .await
        {
            tracing::warn!(%err, "failed to finish tool call");
        }
    }
    let is_error = !outcome.ok;
    let _ = ctx
        .events
        .send(Event::ToolDone {
            id,
            call: ToolCallId(tui_id),
            outcome,
        })
        .await;
    ContentBlock::ToolResult {
        tool_use_id: vendor_id.to_owned(),
        content: cap_tool_result(ctx.redaction.redact(&result_text)),
        is_error,
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
    id: TaskId,
    spec: ToolCallSpec<'_>,
    tool_ctx: &ToolContext,
    ops: &mut mpsc::Receiver<Op>,
    deferred: &mut Vec<Op>,
) -> ToolExecResult {
    let ToolCallSpec {
        vendor_id,
        name,
        input_json,
        tui_id,
        db_id,
    } = spec;
    let run_fut = async {
        match ctx.tools.get(name) {
            Some(tool) => tool.run(input_json, tool_ctx).await,
            None => Err(goat_tool::ToolError::UnknownTool {
                name: name.to_owned(),
            }),
        }
    };
    let mut run_fut = std::pin::pin!(run_fut);
    let tool_result = loop {
        tokio::select! {
            biased;
            maybe_op = ops.recv() => match maybe_op {
                Some(Op::Interrupt { id: task_id }) if task_id == id => break ToolResult::Interrupted,
                Some(Op::Shutdown) | None => break ToolResult::ShuttingDown,
                Some(op) => { deferred.push(op); }
            },
            r = &mut run_fut => break ToolResult::Done(r),
        }
    };
    match tool_result {
        ToolResult::ShuttingDown => ToolExecResult {
            result_content: ContentBlock::ToolResult {
                tool_use_id: vendor_id.to_owned(),
                content: "shutdown".to_owned(),
                is_error: true,
            },
            interrupted: false,
            shutting_down: true,
        },
        ToolResult::Interrupted => {
            let block = finish_tool_interrupted(ctx, id, vendor_id, tui_id, db_id).await;
            ToolExecResult {
                result_content: block,
                interrupted: true,
                shutting_down: false,
            }
        }
        ToolResult::Done(result) => {
            let block = finish_tool_success(ctx, id, vendor_id, tui_id, db_id, &result).await;
            ToolExecResult {
                result_content: block,
                interrupted: false,
                shutting_down: false,
            }
        }
    }
}

async fn run_tool_batch(
    ctx: &Ctx<'_>,
    id: TaskId,
    pending_calls: &[(String, String, String)],
    state: &mut RoundState<'_>,
    tool_ctx: &ToolContext,
    ops: &mut mpsc::Receiver<Op>,
) -> ToolBatchResult {
    let mut tool_results: Vec<ContentBlock> = Vec::new();
    let mut interrupted_at: Option<usize> = None;
    for (index, (vendor_id, name, input_json)) in pending_calls.iter().enumerate() {
        *state.call_seq += 1;
        let tui_id = *state.call_seq;
        let redacted_input = ctx.redaction.redact(input_json);
        let _ = ctx
            .events
            .send(Event::ToolStarted {
                id,
                call: goat_protocol::ToolCall {
                    id: ToolCallId(tui_id),
                    name: name.clone(),
                    input: redacted_input.clone(),
                },
            })
            .await;
        let db_id = if let (Some(tid), Some(turn)) = (state.ids.stored_thread, state.ids.turn_db_id)
        {
            match ctx
                .store
                .create_tool_call(goat_store::NewToolCall {
                    thread_id: tid,
                    turn_id: turn,
                    call_id: vendor_id.clone(),
                    name: name.clone(),
                    input: redacted_input,
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
        } else {
            None
        };
        let spec = ToolCallSpec {
            vendor_id,
            name,
            input_json,
            tui_id,
            db_id,
        };
        let exec = execute_tool(ctx, id, spec, tool_ctx, ops, state.deferred).await;
        if exec.shutting_down {
            return ToolBatchResult {
                tool_results,
                interrupted_at,
                shutting_down: true,
            };
        }
        if exec.interrupted {
            interrupted_at = Some(index);
            tool_results.push(exec.result_content);
            break;
        }
        tool_results.push(exec.result_content);
    }
    if let Some(index) = interrupted_at {
        for (vendor_id, _, _) in pending_calls.iter().skip(index + 1) {
            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: vendor_id.clone(),
                content: "interrupted".to_owned(),
                is_error: true,
            });
        }
    }
    ToolBatchResult {
        tool_results,
        interrupted_at,
        shutting_down: false,
    }
}

async fn ensure_thread(
    store: &Store,
    thread_id: &mut Option<i64>,
    target: &ModelTarget,
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
            title: None,
            provider: target.provider.clone(),
            model: target.model.clone(),
            account: target.account.clone(),
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
    let stored_thread = ensure_thread(ctx.store, thread_id, target).await;
    let turn_db_id = if let Some(tid) = stored_thread {
        if let Err(err) = ctx
            .store
            .create_message(NewMessage {
                thread_id: tid,
                turn_id: None,
                role: "user".to_owned(),
                body: text.to_owned(),
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

async fn persist_assistant_text(ctx: &Ctx<'_>, raw: &str, ids: &TurnIds) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let shown = ctx.redaction.redact(raw);
    if let (Some(tid), Some(turn)) = (ids.stored_thread, ids.turn_db_id)
        && let Err(err) = ctx
            .store
            .create_message(NewMessage {
                thread_id: tid,
                turn_id: Some(turn),
                role: "assistant".to_owned(),
                body: shown.clone(),
                created_at: now_ms(),
            })
            .await
    {
        tracing::warn!(%err, "failed to persist assistant message");
    }
    Some(shown)
}

async fn finalize_turn(ctx: &Ctx<'_>, id: TaskId, outcome: &RoundEnd, ids: &TurnIds) {
    match outcome {
        RoundEnd::Completed => {
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
        RoundEnd::Interrupted => {
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
        RoundEnd::Failed(message) => {
            let _ = ctx
                .events
                .send(Event::Error {
                    id: Some(id),
                    message: ctx.redaction.redact(message),
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
        RoundEnd::ShuttingDown => {}
    }
}

async fn process_round_output(
    ctx: &Ctx<'_>,
    id: TaskId,
    round: RoundResult,
    history: &mut Vec<ProviderMessage>,
    state: &mut RoundState<'_>,
    ops: &mut mpsc::Receiver<Op>,
) -> RoundOutcome {
    let raw = round.raw;
    let pending_calls = round.pending_calls;
    if !raw.is_empty() || !pending_calls.is_empty() {
        let mut content = Vec::new();
        if !raw.is_empty() {
            content.push(ContentBlock::Text {
                text: ctx.redaction.redact(&raw),
            });
        }
        for (vendor_id, name, input_json) in &pending_calls {
            let input_val = serde_json::from_str(input_json).unwrap_or(serde_json::Value::Null);
            content.push(ContentBlock::ToolUse {
                id: vendor_id.clone(),
                name: name.clone(),
                input: input_val,
            });
        }
        history.push(ProviderMessage {
            role: MessageRole::Assistant,
            content,
        });
    }
    let shown_text = persist_assistant_text(ctx, &raw, state.ids).await;
    if pending_calls.is_empty() {
        if let Some(shown) = shown_text {
            let _ = ctx.events.send(Event::TextDone { id, text: shown }).await;
        }
        return RoundOutcome::Done(RoundEnd::Completed);
    }
    if state.rounds >= MAX_TOOL_ROUNDS {
        tracing::warn!(state.rounds, "tool round cap reached; ending turn");
        let synthetic: Vec<ContentBlock> = pending_calls
            .iter()
            .map(|(vendor_id, _, _)| ContentBlock::ToolResult {
                tool_use_id: vendor_id.clone(),
                content: "tool round limit reached".to_owned(),
                is_error: true,
            })
            .collect();
        history.push(ProviderMessage {
            role: MessageRole::User,
            content: synthetic,
        });
        if let Some(shown) = shown_text {
            let _ = ctx.events.send(Event::TextDone { id, text: shown }).await;
        }
        return RoundOutcome::Done(RoundEnd::Completed);
    }
    let tool_ctx = match ToolContext::new(state.cwd) {
        Ok(tool_ctx) => tool_ctx,
        Err(err) => return RoundOutcome::Done(RoundEnd::Failed(err.to_string())),
    };
    let batch = run_tool_batch(ctx, id, &pending_calls, state, &tool_ctx, ops).await;
    if batch.shutting_down {
        return RoundOutcome::Shutdown;
    }
    history.push(ProviderMessage {
        role: MessageRole::User,
        content: batch.tool_results,
    });
    if batch.interrupted_at.is_some() {
        RoundOutcome::Done(RoundEnd::Interrupted)
    } else {
        RoundOutcome::Continue
    }
}

fn build_tool_defs(
    ctx: &Ctx<'_>,
    provider: &dyn goat_provider::ModelProvider,
) -> Vec<ToolDefinition> {
    if !provider.capabilities().tools {
        return Vec::new();
    }
    ctx.tools
        .specs()
        .into_iter()
        .map(|spec| ToolDefinition {
            name: spec.name.to_owned(),
            description: spec.description.to_owned(),
            input_schema: spec.parameters,
        })
        .collect()
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

async fn handle_turn(
    ctx: &Ctx<'_>,
    id: TaskId,
    text: String,
    target: &mut Option<ModelTarget>,
    history: &mut Vec<ProviderMessage>,
    thread_id: &mut Option<i64>,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    let Some(resolved) = target.as_ref() else {
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
            SYSTEM_PROMPT.to_owned(),
        ));
    }
    history.push(ProviderMessage::text(MessageRole::User, text.clone()));
    let ids = init_db_turn(ctx, id, &text, resolved, thread_id).await;
    if ctx.events.send(Event::TaskStarted { id }).await.is_err() {
        finalize_turn(ctx, id, &RoundEnd::ShuttingDown, &ids).await;
        return Flow::Shutdown;
    }

    let turn_model = resolved.model.clone();
    let tool_defs = build_tool_defs(ctx, provider.as_ref());
    let cwd = resolve_thread_cwd(ctx, ids.stored_thread).await;
    let mut call_seq: u64 = 0;
    let mut rounds = 0usize;
    let mut deferred: Vec<Op> = Vec::new();
    let outcome = loop {
        rounds += 1;
        let request = ModelRequest {
            model: turn_model.clone(),
            messages: history.clone(),
            tools: tool_defs.clone(),
        };
        let round = run_round(
            id,
            provider.as_ref(),
            request,
            ops,
            ctx.events,
            ctx.redaction,
            &mut deferred,
        )
        .await;
        match round.end {
            RoundEnd::ShuttingDown => {
                finalize_turn(ctx, id, &RoundEnd::ShuttingDown, &ids).await;
                return Flow::Shutdown;
            }
            RoundEnd::Interrupted => break RoundEnd::Interrupted,
            RoundEnd::Failed(message) => break RoundEnd::Failed(message),
            RoundEnd::Completed => {}
        }
        let mut state = RoundState {
            ids: &ids,
            rounds,
            cwd: &cwd,
            call_seq: &mut call_seq,
            deferred: &mut deferred,
        };
        match process_round_output(ctx, id, round, history, &mut state, ops).await {
            RoundOutcome::Done(end) => break end,
            RoundOutcome::Shutdown => {
                finalize_turn(ctx, id, &RoundEnd::ShuttingDown, &ids).await;
                return Flow::Shutdown;
            }
            RoundOutcome::Continue => {}
        }
    };

    finalize_turn(ctx, id, &outcome, &ids).await;

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

    fn target(provider: &str) -> ModelTarget {
        ModelTarget {
            provider: provider.to_owned(),
            model: "m".to_owned(),
            account: "default".to_owned(),
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
}
