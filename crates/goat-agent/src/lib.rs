use std::{
    collections::HashSet,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use goat_auth::{CredentialKey, CredentialStore, RedactionSet, ResolvedCredential, SecretString};
use goat_core::Engine;
use goat_protocol::{
    AccountChoice, Event, LoginCredential, LoginProvider, ModelEntry, ModelTarget, Op, Severity,
    TaskId, ToolCallId,
};
use goat_provider::{
    ContentBlock, MessageRole, ModelEvent, ModelRequest, ProviderId, ProviderMessage,
    ToolDefinition,
};
use goat_providers::{DEFAULT_ACCOUNT, Registry};
use goat_store::{NewMessage, NewThread, NewTurn, Store};
use goat_tool::{ToolContext, outcome_from};
use goat_tools::ToolRegistry;
use tokio::{sync::mpsc, task::JoinHandle};

const MAX_TOOL_ROUNDS: usize = 20;

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
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

fn to_protocol_auth(method: goat_provider::AuthMethod) -> goat_protocol::AuthMethod {
    match method {
        goat_provider::AuthMethod::None => goat_protocol::AuthMethod::None,
        goat_provider::AuthMethod::ApiKey => goat_protocol::AuthMethod::ApiKey,
        goat_provider::AuthMethod::OAuth => goat_protocol::AuthMethod::OAuth,
    }
}

async fn restore_target(store: &Store, registry: &Registry) -> Option<ModelTarget> {
    let cwd = std::env::current_dir()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let thread = store.latest_thread_in(cwd).await.ok().flatten()?;
    registry.get(&ProviderId::from(thread.provider.as_str()))?;
    Some(ModelTarget {
        provider: thread.provider,
        model: thread.model,
        account: thread.account,
    })
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
        target = restore_target(&store, &registry).await;
    }
    announce_startup(&events, &registry, target.as_ref()).await;

    while let Some(op) = ops.recv().await {
        match op {
            Op::SubmitMessage { id, text } => {
                let ctx = Ctx {
                    registry: &registry,
                    tools: &tools,
                    store: &store,
                    redaction: &redaction,
                    events: &events,
                };
                if let Flow::Shutdown = handle_turn(
                    &ctx,
                    id,
                    text,
                    target.as_ref(),
                    &mut history,
                    &mut thread_id,
                    &mut ops,
                )
                .await
                {
                    break;
                }
            }
            Op::Interrupt { .. } => {}
            Op::SelectModel { target: chosen } => {
                if let Some(tid) = thread_id {
                    let _ = store
                        .update_thread_model(
                            tid,
                            chosen.provider.clone(),
                            chosen.model.clone(),
                            chosen.account.clone(),
                            now(),
                        )
                        .await;
                }
                target = Some(chosen.clone());
                let _ = events.send(Event::ModelSelected { target: chosen }).await;
            }
            Op::RefreshModels => {
                let entries = discover_ready(&registry).await;
                let _ = events.send(Event::ModelListChanged { entries }).await;
            }
            Op::Login {
                provider,
                credential,
            } => {
                handle_login(
                    provider,
                    credential,
                    &credentials,
                    &mut registry,
                    &mut redaction,
                    &events,
                )
                .await;
            }
            Op::Shutdown => break,
        }
    }
}

async fn announce_startup(
    events: &mpsc::Sender<Event>,
    registry: &Registry,
    target: Option<&ModelTarget>,
) {
    let _ = events
        .send(Event::LoginProviders {
            providers: registry
                .login_providers()
                .into_iter()
                .map(|(id, method)| LoginProvider {
                    id,
                    method: to_protocol_auth(method),
                })
                .collect(),
        })
        .await;
    let _ = events
        .send(Event::ModelListChanged {
            entries: catalog_only(registry),
        })
        .await;
    let entries = discover_ready(registry).await;
    let _ = events.send(Event::ModelListChanged { entries }).await;
    if let Some(selected) = target {
        let _ = events
            .send(Event::ModelSelected {
                target: selected.clone(),
            })
            .await;
    }
}

async fn handle_login(
    provider: String,
    credential: LoginCredential,
    credentials: &CredentialStore,
    registry: &mut Registry,
    redaction: &mut RedactionSet,
    events: &mpsc::Sender<Event>,
) {
    let key = CredentialKey {
        provider: provider.clone(),
        account: DEFAULT_ACCOUNT.to_owned(),
    };
    let stored = match credential {
        LoginCredential::ApiKey(secret) => {
            redaction.insert(&secret);
            credentials
                .store(&key, ResolvedCredential::ApiKey(SecretString::from(secret)))
                .map_err(|err| err.to_string())
        }
        LoginCredential::OAuth => {
            let (status_tx, mut status_rx) = mpsc::channel::<String>(8);
            let status_provider = provider.clone();
            let status_events = events.clone();
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
            let result = goat_providers::oauth_login(&provider, &status_tx).await;
            drop(status_tx);
            let _ = forwarder.await;
            match result {
                Ok(tokens) => {
                    redaction.insert(tokens.access_token.expose());
                    if let Some(refresh) = &tokens.refresh_token {
                        redaction.insert(refresh.expose());
                    }
                    credentials
                        .store(&key, ResolvedCredential::OAuth(tokens))
                        .map_err(|err| err.to_string())
                }
                Err(err) => Err(err),
            }
        }
    };
    match stored {
        Ok(()) => {
            *registry = Registry::for_account(credentials, DEFAULT_ACCOUNT);
            let entries = discover_ready(registry).await;
            let provider_count = entries
                .iter()
                .filter(|entry| entry.provider == provider)
                .count();
            let _ = events.send(Event::ModelListChanged { entries }).await;
            let (ok, message) = if provider_count > 0 {
                (
                    true,
                    format!("{provider}: {provider_count} models available"),
                )
            } else {
                (
                    false,
                    format!("{provider}: no models available — check credentials"),
                )
            };
            let _ = events
                .send(Event::LoginStatus {
                    provider,
                    message,
                    done: true,
                    ok,
                })
                .await;
        }
        Err(message) => {
            let _ = events
                .send(Event::LoginStatus {
                    provider,
                    message,
                    done: true,
                    ok: false,
                })
                .await;
        }
    }
}

fn catalog_only(registry: &Registry) -> Vec<ModelEntry> {
    let mut entries = Vec::new();
    for provider in registry.all() {
        if !provider.authenticated() {
            continue;
        }
        let provider_id = provider.id().to_string();
        for &id in provider.catalog() {
            let target = ModelTarget {
                provider: provider_id.clone(),
                model: id.to_owned(),
                account: DEFAULT_ACCOUNT.to_owned(),
            };
            entries.push(ModelEntry {
                provider: provider_id.clone(),
                model: id.to_owned(),
                accounts: vec![AccountChoice {
                    id: DEFAULT_ACCOUNT.to_owned(),
                    display: DEFAULT_ACCOUNT.to_owned(),
                    target,
                }],
            });
        }
    }
    entries
}

async fn discover_ready(registry: &Registry) -> Vec<ModelEntry> {
    let mut entries = Vec::new();
    for provider in registry.all() {
        if !provider.authenticated() {
            continue;
        }
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
            let target = ModelTarget {
                provider: provider_id.clone(),
                model: id.to_owned(),
                account: DEFAULT_ACCOUNT.to_owned(),
            };
            entries.push(ModelEntry {
                provider: provider_id.clone(),
                model: id.to_owned(),
                accounts: vec![AccountChoice {
                    id: DEFAULT_ACCOUNT.to_owned(),
                    display: DEFAULT_ACCOUNT.to_owned(),
                    target,
                }],
            });
        }

        for info in discovered {
            if catalog_ids.contains(info.id.as_str()) {
                continue;
            }
            let target = ModelTarget {
                provider: provider_id.clone(),
                model: info.id.clone(),
                account: DEFAULT_ACCOUNT.to_owned(),
            };
            entries.push(ModelEntry {
                provider: provider_id.clone(),
                model: info.id,
                accounts: vec![AccountChoice {
                    id: DEFAULT_ACCOUNT.to_owned(),
                    display: DEFAULT_ACCOUNT.to_owned(),
                    target,
                }],
            });
        }
    }
    entries
}

async fn ensure_thread(
    store: &Store,
    thread_id: &mut Option<i64>,
    target: &ModelTarget,
) -> Option<i64> {
    if let Some(tid) = thread_id {
        return Some(*tid);
    }
    let timestamp = now();
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

#[allow(clippy::too_many_lines)]
async fn handle_turn(
    ctx: &Ctx<'_>,
    id: TaskId,
    text: String,
    target: Option<&ModelTarget>,
    history: &mut Vec<ProviderMessage>,
    thread_id: &mut Option<i64>,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    let Some(target) = target else {
        let _ = ctx
            .events
            .send(Event::Error {
                id: Some(id),
                severity: Severity::Recoverable,
                message: "no model selected".to_owned(),
            })
            .await;
        let _ = ctx
            .events
            .send(Event::TaskDone {
                id,
                interrupted: true,
            })
            .await;
        return Flow::Continue;
    };
    let Some(provider) = ctx
        .registry
        .get(&ProviderId::from(target.provider.as_str()))
    else {
        let _ = ctx
            .events
            .send(Event::Error {
                id: Some(id),
                severity: Severity::Recoverable,
                message: format!("unknown provider: {}", target.provider),
            })
            .await;
        let _ = ctx
            .events
            .send(Event::TaskDone {
                id,
                interrupted: true,
            })
            .await;
        return Flow::Continue;
    };

    history.push(ProviderMessage::text(MessageRole::User, text.clone()));

    let stored_thread = ensure_thread(ctx.store, thread_id, target).await;
    let turn_db_id = if let Some(tid) = stored_thread {
        let _ = ctx
            .store
            .create_message(NewMessage {
                thread_id: tid,
                turn_id: None,
                role: "user".to_owned(),
                body: text,
                created_at: now(),
            })
            .await;
        ctx.store
            .create_turn(NewTurn {
                thread_id: tid,
                task_id: i64::try_from(id.0).unwrap_or(0),
                provider: target.provider.clone(),
                model: target.model.clone(),
                account: target.account.clone(),
                status: "running".to_owned(),
                started_at: now(),
            })
            .await
            .ok()
    } else {
        None
    };

    if ctx.events.send(Event::TaskStarted { id }).await.is_err() {
        return Flow::Shutdown;
    }

    let supports_tools = provider.capabilities().tools;
    let tool_defs = if supports_tools {
        ctx.tools
            .specs()
            .into_iter()
            .map(|spec| ToolDefinition {
                name: spec.name.to_owned(),
                description: spec.description.to_owned(),
                input_schema: spec.parameters,
            })
            .collect()
    } else {
        Vec::new()
    };

    let cwd = match stored_thread {
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
    };

    let mut call_seq: u64 = 0;
    let mut rounds = 0usize;

    let outcome = loop {
        rounds += 1;

        let request = ModelRequest {
            model: target.model.clone(),
            messages: history.clone(),
            tools: tool_defs.clone(),
        };
        let (mev_tx, mut mev_rx) = mpsc::channel(64);
        let handle = provider.request(request, mev_tx);

        let mut raw = String::new();
        let mut pending_calls: Vec<(String, String, String)> = Vec::new();

        let round_end = loop {
            tokio::select! {
                biased;
                maybe_op = ops.recv() => match maybe_op {
                    Some(Op::Interrupt { .. }) => {
                        handle.abort();
                        break RoundEnd::Interrupted;
                    }
                    Some(Op::Shutdown) | None => {
                        handle.abort();
                        return Flow::Shutdown;
                    }
                    Some(_) => {}
                },
                maybe_event = mev_rx.recv() => match maybe_event {
                    Some(ModelEvent::TextDelta { text }) => {
                        raw.push_str(&text);
                        let shown = ctx.redaction.redact(&text);
                        if ctx.events.send(Event::TextDelta { id, chunk: shown }).await.is_err() {
                            handle.abort();
                            return Flow::Shutdown;
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

        match round_end {
            RoundEnd::Interrupted => break RoundEnd::Interrupted,
            RoundEnd::Failed(message) => break RoundEnd::Failed(message),
            RoundEnd::Completed => {}
        }

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

        if !raw.is_empty() {
            let shown = ctx.redaction.redact(&raw);
            let _ = ctx
                .events
                .send(Event::TextDone {
                    id,
                    text: shown.clone(),
                })
                .await;
            if let (Some(tid), Some(turn)) = (stored_thread, turn_db_id) {
                let _ = ctx
                    .store
                    .create_message(NewMessage {
                        thread_id: tid,
                        turn_id: Some(turn),
                        role: "assistant".to_owned(),
                        body: shown,
                        created_at: now(),
                    })
                    .await;
            }
        }

        if pending_calls.is_empty() {
            break RoundEnd::Completed;
        }

        if rounds >= MAX_TOOL_ROUNDS {
            tracing::warn!(rounds, "tool round cap reached; ending turn");
            break RoundEnd::Completed;
        }

        let tool_ctx = match ToolContext::new(&cwd) {
            Ok(tool_ctx) => tool_ctx,
            Err(err) => break RoundEnd::Failed(err.to_string()),
        };

        let mut tool_results: Vec<ContentBlock> = Vec::new();
        for (vendor_id, name, input_json) in pending_calls {
            call_seq += 1;
            let tui_id = call_seq;

            let _ = ctx
                .events
                .send(Event::ToolStarted {
                    id,
                    call: goat_protocol::ToolCall {
                        id: ToolCallId(tui_id),
                        name: name.clone(),
                        input: input_json.clone(),
                    },
                })
                .await;

            let db_id = if let (Some(tid), Some(turn)) = (stored_thread, turn_db_id) {
                ctx.store
                    .create_tool_call(goat_store::NewToolCall {
                        thread_id: tid,
                        turn_id: turn,
                        call_id: vendor_id.clone(),
                        name: name.clone(),
                        input: input_json.clone(),
                        status: "running".to_owned(),
                        started_at: now_ms(),
                    })
                    .await
                    .ok()
            } else {
                None
            };

            let result = match ctx.tools.get(&name) {
                Some(tool) => tool.run(&input_json, &tool_ctx).await,
                None => Err(goat_tool::ToolError::UnknownTool { name: name.clone() }),
            };
            let (outcome, result_text) = outcome_from(&result);

            if let Some(db) = db_id {
                let status = if outcome.ok { "done" } else { "error" }.to_owned();
                let _ = ctx
                    .store
                    .finish_tool_call(db, status, outcome.summary.clone(), now_ms())
                    .await;
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

            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: vendor_id,
                content: ctx.redaction.redact(&result_text),
                is_error,
            });
        }

        history.push(ProviderMessage {
            role: MessageRole::User,
            content: tool_results,
        });
    };

    match outcome {
        RoundEnd::Completed => {
            if let Some(turn) = turn_db_id {
                let _ = ctx.store.finish_turn(turn, "done".to_owned(), now()).await;
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
            if let Some(turn) = turn_db_id {
                let _ = ctx
                    .store
                    .finish_turn(turn, "interrupted".to_owned(), now())
                    .await;
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
                    severity: Severity::Recoverable,
                    message: ctx.redaction.redact(&message),
                })
                .await;
            if let Some(turn) = turn_db_id {
                let _ = ctx.store.finish_turn(turn, "error".to_owned(), now()).await;
            }
            let _ = ctx
                .events
                .send(Event::TaskDone {
                    id,
                    interrupted: true,
                })
                .await;
        }
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
                streaming: true,
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
        let mut session = Session::spawn(agent_with("hello", 0));
        session
            .ops()
            .send(Op::SubmitMessage {
                id: TaskId(1),
                text: "hi".to_owned(),
            })
            .await
            .unwrap();

        let mut started = false;
        let mut deltas = String::new();
        let mut done = false;
        while let Some(event) = session.next_event().await {
            match event {
                Event::ModelListChanged { .. }
                | Event::ModelSelected { .. }
                | Event::LoginProviders { .. }
                | Event::LoginStatus { .. }
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
        let mut session = Session::spawn(agent_with("late", 5_000));
        let ops = session.ops();
        ops.send(Op::SubmitMessage {
            id: TaskId(9),
            text: "hi".to_owned(),
        })
        .await
        .unwrap();
        ops.send(Op::Interrupt { id: TaskId(9) }).await.unwrap();

        let mut interrupted = false;
        while let Some(event) = session.next_event().await {
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
        let mut session = Session::spawn(agent);
        session
            .ops()
            .send(Op::SubmitMessage {
                id: TaskId(1),
                text: "hi".to_owned(),
            })
            .await
            .unwrap();

        let mut saw_error = false;
        while let Some(event) = session.next_event().await {
            match event {
                Event::Error { .. } => saw_error = true,
                Event::TaskDone { .. } => break,
                _ => {}
            }
        }
        assert!(saw_error);
    }
}
