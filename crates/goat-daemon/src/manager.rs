use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use goat_agent::GoatAgent;
use goat_auth::CredentialStore;
use goat_core::Session;
use goat_protocol::Op;
use goat_store::Store;
use goat_wire::{ClientId, ResumeMode, ServerFrame, SessionId, SessionInfo};
use tokio::sync::Mutex;
use tokio::sync::mpsc;

use crate::session::{LiveSession, SessionInner, SessionTable};

#[derive(Clone)]
pub(crate) struct Manager {
    inner: Arc<ManagerInner>,
}

struct ManagerInner {
    auth_path: PathBuf,
    db_path: PathBuf,
    sessions: Mutex<SessionTable>,
    next_session: AtomicU64,
    next_client: AtomicU64,
}

impl Manager {
    pub(crate) fn new(auth_path: PathBuf, db_path: PathBuf) -> Self {
        Self {
            inner: Arc::new(ManagerInner {
                auth_path,
                db_path,
                sessions: Mutex::new(HashMap::new()),
                next_session: AtomicU64::new(1),
                next_client: AtomicU64::new(1),
            }),
        }
    }

    pub(crate) fn next_client_id(&self) -> ClientId {
        ClientId(self.inner.next_client.fetch_add(1, Ordering::Relaxed))
    }

    fn now_ms() -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }

    fn normalize_cwd(cwd: &std::path::Path) -> String {
        std::fs::canonicalize(cwd)
            .unwrap_or_else(|_| cwd.to_path_buf())
            .display()
            .to_string()
    }

    fn find_live_by_cwd_locked(table: &SessionTable, cwd: &str) -> Option<SessionId> {
        for (id, live) in table {
            if live.cwd == cwd {
                return Some(*id);
            }
        }
        None
    }

    pub(crate) async fn open_or_attach(
        &self,
        cwd: PathBuf,
        resume: ResumeMode,
    ) -> Result<SessionId, String> {
        let normalized = Self::normalize_cwd(&cwd);
        if matches!(resume, ResumeMode::Latest) {
            let table = self.inner.sessions.lock().await;
            if let Some(id) = Self::find_live_by_cwd_locked(&table, &normalized) {
                return Ok(id);
            }
        }
        self.open_session(cwd, normalized, resume).await
    }

    async fn open_session(
        &self,
        cwd: PathBuf,
        normalized: String,
        resume: ResumeMode,
    ) -> Result<SessionId, String> {
        let credentials = CredentialStore::new(self.inner.auth_path.clone());
        let store = Store::open(&self.inner.db_path).map_err(|e| format!("store: {e}"))?;
        let store_for_pump = store.clone();
        let registry = goat_providers::Registry::new(&credentials);
        let agent = GoatAgent::new(registry, store, credentials, None, cwd.clone()).await;
        let session = Session::spawn(agent);
        let (ops, events, handle) = session.into_parts();

        let id = SessionId(self.inner.next_session.fetch_add(1, Ordering::Relaxed));
        let inner = Arc::new(Mutex::new(SessionInner {
            id,
            cwd: normalized.clone(),
            created_at: Self::now_ms(),
            ops: ops.clone(),
            log: std::collections::VecDeque::new(),
            next_seq: 0,
            next_task: 1,
            subscribers: Vec::new(),
            state: goat_wire::SessionLiveState::Idle,
            snapshot: None,
            tokens: 0,
            open_asks: 0,
            thread_id: None,
            resurrected: std::collections::HashSet::new(),
        }));

        let id = {
            let mut table = self.inner.sessions.lock().await;
            if matches!(resume, ResumeMode::Latest)
                && let Some(existing) = Self::find_live_by_cwd_locked(&table, &normalized)
            {
                let _ = ops.send(Op::Shutdown).await;
                return Ok(existing);
            }
            table.insert(
                id,
                LiveSession {
                    cwd: normalized,
                    inner: inner.clone(),
                },
            );
            id
        };

        spawn_pump(self.clone(), id, inner, events, handle, store_for_pump);

        match resume {
            ResumeMode::New => {}
            ResumeMode::Latest => {
                let _ = ops.send(Op::ResumeLatest).await;
            }
            ResumeMode::Thread { thread_id } => {
                let _ = ops.send(Op::Resume { thread_id }).await;
            }
        }
        Ok(id)
    }

    pub(crate) async fn subscribe(
        &self,
        session: SessionId,
        client: ClientId,
        sender: mpsc::Sender<ServerFrame>,
    ) -> Result<(), String> {
        let live = {
            let table = self.inner.sessions.lock().await;
            table.get(&session).cloned()
        };
        let live = live.ok_or("unknown session")?;
        let mut inner = live.inner.lock().await;
        if let Some(snap) = inner.snapshot.clone() {
            let _ = sender
                .send(ServerFrame::Snapshot {
                    session,
                    watermark: snap.watermark,
                    target: snap.target,
                    entries: snap.entries,
                    context_tokens: snap.context_tokens,
                    compaction_threshold: snap.compaction_threshold,
                    mode: snap.mode,
                })
                .await;
        }
        for (seq, event) in &inner.log {
            if let Some(snap) = &inner.snapshot
                && *seq < snap.watermark
            {
                continue;
            }
            let _ = sender
                .send(ServerFrame::Event {
                    session,
                    seq: *seq,
                    event: event.clone(),
                })
                .await;
        }
        crate::session::subscriber_upsert(&mut inner.subscribers, client, sender);
        let clients = inner.presence();
        broadcast_presence(&mut inner, clients);
        Ok(())
    }

    pub(crate) async fn unsubscribe(&self, session: SessionId, client: ClientId) {
        let live = {
            let table = self.inner.sessions.lock().await;
            table.get(&session).cloned()
        };
        let mut evict = false;
        if let Some(live) = live {
            let mut inner = live.inner.lock().await;
            crate::session::subscriber_map_remove(&mut inner.subscribers, client);
            let clients = inner.presence();
            broadcast_presence(&mut inner, clients);
            evict = inner.evictable();
        }
        if evict {
            self.evict_if_idle(session).await;
        }
    }

    pub(crate) async fn drop_client(&self, client: ClientId) {
        let lives: Vec<(SessionId, LiveSession)> = {
            let table = self.inner.sessions.lock().await;
            table.iter().map(|(id, live)| (*id, live.clone())).collect()
        };
        let mut candidates = Vec::new();
        for (id, live) in lives {
            let mut inner = live.inner.lock().await;
            crate::session::subscriber_map_remove(&mut inner.subscribers, client);
            let clients = inner.presence();
            broadcast_presence(&mut inner, clients);
            if inner.evictable() {
                candidates.push(id);
            }
        }
        for id in candidates {
            self.evict_if_idle(id).await;
        }
    }

    async fn evict_if_idle(&self, session: SessionId) {
        let ops = {
            let mut table = self.inner.sessions.lock().await;
            let Some(live) = table.get(&session) else {
                return;
            };
            let inner = live.inner.lock().await;
            if !inner.evictable() {
                return;
            }
            let ops = inner.ops.clone();
            drop(inner);
            table.remove(&session);
            ops
        };
        let _ = ops.send(Op::Shutdown).await;
        tracing::info!(session = session.0, "evicted idle session with no windows");
    }

    pub(crate) async fn submit(
        &self,
        session: SessionId,
        client_sender: &mpsc::Sender<ServerFrame>,
        correlation: u64,
        mut op: Op,
    ) -> Result<(), String> {
        let live = {
            let table = self.inner.sessions.lock().await;
            table.get(&session).cloned()
        };
        let live = live.ok_or("unknown session")?;
        let (ops, task) = {
            let mut inner = live.inner.lock().await;
            let task = inner.allocate_task();
            (inner.ops.clone(), task)
        };
        match &mut op {
            Op::SubmitMessage { id, .. } | Op::SubmitShell { id, .. } | Op::Compact { id, .. } => {
                *id = task;
            }
            _ => {}
        }
        let _ = client_sender
            .send(ServerFrame::CorrelationAssigned {
                session,
                correlation,
                task,
            })
            .await;
        ops.send(op).await.map_err(|_| "engine closed".to_owned())
    }

    pub(crate) async fn control(&self, session: SessionId, op: Op) -> Result<(), String> {
        let live = {
            let table = self.inner.sessions.lock().await;
            table.get(&session).cloned()
        };
        let live = live.ok_or("unknown session")?;
        let (ops, thread_id, rewritten) = {
            let mut inner = live.inner.lock().await;
            let rewritten = rewrite_resurrected_answer(&mut inner, &op);
            (inner.ops.clone(), inner.thread_id, rewritten)
        };
        if let Some((call, message)) = rewritten {
            if let Some(tid) = thread_id {
                let store = Store::open(&self.inner.db_path).map_err(|e| format!("store: {e}"))?;
                let _ = store.clear_open_prompt(tid, format!("{call}")).await;
            }
            return ops
                .send(Op::SubmitMessage {
                    id: goat_protocol::TaskId(0),
                    text: message,
                })
                .await
                .map_err(|_| "engine closed".to_owned());
        }
        ops.send(op).await.map_err(|_| "engine closed".to_owned())
    }

    pub(crate) async fn list_sessions(&self) -> Vec<SessionInfo> {
        let lives: Vec<LiveSession> = {
            let table = self.inner.sessions.lock().await;
            table.values().cloned().collect()
        };
        let mut out = Vec::new();
        let now = Self::now_ms();
        for live in lives {
            let inner = live.inner.lock().await;
            out.push(SessionInfo {
                session: inner.id,
                cwd: inner.cwd.clone(),
                state: inner.state,
                windows: inner.subscribers.len(),
                age_ms: now - inner.created_at,
                tokens: inner.tokens,
            });
        }
        out
    }

    pub(crate) async fn kill_session(&self, session: SessionId) -> Result<(), String> {
        let live = {
            let mut table = self.inner.sessions.lock().await;
            table.remove(&session)
        };
        let live = live.ok_or("unknown session")?;
        let ops = {
            let inner = live.inner.lock().await;
            inner.ops.clone()
        };
        let _ = ops.send(Op::Shutdown).await;
        Ok(())
    }

    async fn remove_session(&self, session: SessionId) {
        let mut table = self.inner.sessions.lock().await;
        table.remove(&session);
    }
}

fn rewrite_resurrected_answer(inner: &mut SessionInner, op: &Op) -> Option<(u64, String)> {
    let (call, message) = match op {
        Op::Answer { call, answers, .. } => (call.0, format!("My answer: {}", answers.join(", "))),
        Op::ResolvePlan { call, decision, .. } => (call.0, format!("Plan decision: {decision:?}")),
        _ => return None,
    };
    if inner.resurrected.remove(&call) {
        inner.open_asks = inner.open_asks.saturating_sub(1);
        Some((call, message))
    } else {
        None
    }
}

fn broadcast_presence(inner: &mut SessionInner, clients: Vec<ClientId>) {
    let frame = ServerFrame::Presence {
        session: inner.id,
        clients,
    };
    inner
        .subscribers
        .retain(|sub| sub.sender.try_send(frame.clone()).is_ok());
}

fn spawn_pump(
    manager: Manager,
    session: SessionId,
    inner: Arc<Mutex<SessionInner>>,
    mut events: mpsc::Receiver<goat_protocol::Event>,
    handle: tokio::task::JoinHandle<()>,
    store: Store,
) {
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let bound = matches!(event, goat_protocol::Event::ThreadBound { .. });
            let persist = {
                let mut guard = inner.lock().await;
                guard.record_and_fanout(event)
            };
            if let Some(persist) = persist {
                let now = Manager::now_ms();
                if let Err(err) = store
                    .append_session_event(persist.thread_id, persist.body, now)
                    .await
                {
                    tracing::warn!(%err, "failed to persist session event");
                }
                match persist.prompt {
                    Some(crate::session::PromptAction::Open {
                        call_id,
                        kind,
                        payload,
                        task_id,
                    }) => {
                        let _ = store
                            .record_open_prompt(
                                persist.thread_id,
                                call_id,
                                kind,
                                payload,
                                task_id,
                                now,
                            )
                            .await;
                    }
                    Some(crate::session::PromptAction::Close { call_id }) => {
                        let _ = store.clear_open_prompt(persist.thread_id, call_id).await;
                    }
                    None => {}
                }
                if bound {
                    resurrect_open_prompts(&inner, &store, persist.thread_id).await;
                }
            }
        }
        {
            let mut guard = inner.lock().await;
            let frame = ServerFrame::Error {
                message: "session engine stopped".to_owned(),
            };
            guard
                .subscribers
                .retain(|sub| sub.sender.try_send(frame.clone()).is_ok());
        }
        manager.remove_session(session).await;
        handle.abort();
    });
}

async fn resurrect_open_prompts(inner: &Arc<Mutex<SessionInner>>, store: &Store, thread_id: i64) {
    let already_live = {
        let guard = inner.lock().await;
        guard.open_asks > 0
    };
    if already_live {
        return;
    }
    let Ok(prompts) = store.open_prompts(thread_id).await else {
        return;
    };
    for prompt in prompts {
        let Ok(call) = prompt.call_id.parse::<u64>() else {
            continue;
        };
        let event = match prompt.kind.as_str() {
            "ask" => {
                let Ok(questions) =
                    serde_json::from_str::<Vec<goat_protocol::AskQuestion>>(&prompt.payload)
                else {
                    continue;
                };
                goat_protocol::Event::AskStarted {
                    id: goat_protocol::TaskId(prompt.task_id),
                    call: goat_protocol::ToolCallId(call),
                    questions,
                }
            }
            "plan" => {
                let Ok((plan, path)) = serde_json::from_str::<(String, String)>(&prompt.payload)
                else {
                    continue;
                };
                goat_protocol::Event::PlanProposed {
                    id: goat_protocol::TaskId(prompt.task_id),
                    call: goat_protocol::ToolCallId(call),
                    plan,
                    path,
                }
            }
            _ => continue,
        };
        let mut guard = inner.lock().await;
        guard.resurrected.insert(call);
        let _ = guard.record_and_fanout(event);
    }
}
