use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;

use goat_protocol::{Event, Op};
use goat_wire::{ClientId, ServerFrame, SessionId, SessionLiveState};
use tokio::sync::Mutex;
use tokio::sync::mpsc;

pub(crate) struct Subscriber {
    pub(crate) client: ClientId,
    pub(crate) sender: mpsc::Sender<ServerFrame>,
}

pub(crate) struct SessionInner {
    pub(crate) id: SessionId,
    pub(crate) cwd: String,
    pub(crate) created_at: i64,
    pub(crate) ops: mpsc::Sender<Op>,
    pub(crate) log: VecDeque<(u64, Event)>,
    pub(crate) next_seq: u64,
    pub(crate) next_task: u64,
    pub(crate) subscribers: Vec<Subscriber>,
    pub(crate) state: SessionLiveState,
    pub(crate) snapshot: Option<RestoredSnapshot>,
    pub(crate) tokens: u64,
    pub(crate) open_asks: usize,
    pub(crate) thread_id: Option<i64>,
    pub(crate) awaits_restore: bool,
    pub(crate) ready: Arc<tokio::sync::Notify>,
    pub(crate) resurrected: std::collections::HashSet<u64>,
    pub(crate) pending_attaches: usize,
}

#[derive(Clone)]
pub(crate) struct RestoredSnapshot {
    pub(crate) watermark: u64,
    pub(crate) target: Option<goat_protocol::ModelTarget>,
    pub(crate) entries: Vec<goat_protocol::TranscriptEntry>,
    pub(crate) context_tokens: Option<u32>,
    pub(crate) compaction_threshold: Option<u32>,
}

#[derive(Clone)]
pub(crate) struct LiveSession {
    pub(crate) inner: Arc<Mutex<SessionInner>>,
}

pub(crate) struct PersistEvent {
    pub(crate) thread_id: i64,
    pub(crate) body: String,
    pub(crate) prompt: Option<PromptAction>,
}

pub(crate) enum PromptAction {
    Open {
        call_id: String,
        kind: String,
        payload: String,
        task_id: u64,
    },
    Close {
        call_id: String,
    },
}

impl SessionInner {
    pub(crate) fn allocate_task(&mut self) -> goat_protocol::TaskId {
        let id = self.next_task;
        self.next_task += 1;
        goat_protocol::TaskId(id)
    }

    pub(crate) fn record_and_fanout(&mut self, event: Event) -> Option<PersistEvent> {
        update_state_from_event(&mut self.state, &event);
        match &event {
            Event::AskStarted { .. } => self.open_asks += 1,
            Event::AskDismissed { .. } => {
                self.open_asks = self.open_asks.saturating_sub(1);
            }
            Event::Usage { usage, .. } => {
                self.tokens = self
                    .tokens
                    .saturating_add(u64::from(usage.input_tokens))
                    .saturating_add(u64::from(usage.output_tokens));
            }
            Event::ThreadBound { thread_id } => self.thread_id = Some(*thread_id),
            _ => {}
        }
        if let Event::ConversationRestored {
            target,
            entries,
            context_tokens,
            compaction_threshold,
        } = &event
        {
            self.snapshot = Some(RestoredSnapshot {
                watermark: self.next_seq + 1,
                target: Some(target.clone()),
                entries: entries.clone(),
                context_tokens: *context_tokens,
                compaction_threshold: *compaction_threshold,
            });
            self.awaits_restore = false;
            self.ready.notify_waiters();
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.log.len() >= MAX_RETAINED_EVENTS {
            self.log.pop_front();
        }
        let prompt = prompt_action(&event);
        let body = self.thread_id.zip(serde_json::to_string(&event).ok());
        let frame = ServerFrame::Event {
            session: self.id,
            seq,
            event: event.clone(),
        };
        self.log.push_back((seq, event));
        self.subscribers
            .retain(|sub| sub.sender.try_send(frame.clone()).is_ok());
        thread_id.map(|thread_id| PersistEvent { thread_id, prompt })
    }

    pub(crate) fn presence(&self) -> Vec<ClientId> {
        self.subscribers.iter().map(|s| s.client).collect()
    }

    pub(crate) fn evictable(&self) -> bool {
        self.subscribers.is_empty()
            && self.pending_attaches == 0
            && self.open_asks == 0
            && matches!(self.state, SessionLiveState::Idle {})
    }
}

const MAX_RETAINED_EVENTS: usize = 4096;

fn update_state_from_event(state: &mut SessionLiveState, event: &Event) {
    match event {
        Event::TaskStarted { .. } | Event::AskDismissed { .. } => {
            *state = SessionLiveState::Active {};
        }
        Event::AskStarted { .. } => *state = SessionLiveState::WaitingOnAsk {},
        Event::TaskDone { .. } => *state = SessionLiveState::Idle {},
        _ => {}
    }
}

fn prompt_action(event: &Event) -> Option<PromptAction> {
    match event {
        Event::AskStarted {
            id,
            call,
            questions,
        } => Some(PromptAction::Open {
            call_id: format!("{}", call.0),
            kind: "ask".to_owned(),
            payload: serde_json::to_string(questions).unwrap_or_default(),
            task_id: id.0,
        }),
        Event::AskDismissed { call, .. } => Some(PromptAction::Close {
            call_id: format!("{}", call.0),
        }),
        _ => None,
    }
}

pub(crate) fn subscriber_map_remove(subs: &mut Vec<Subscriber>, client: ClientId) {
    subs.retain(|s| s.client != client);
}

pub(crate) fn subscriber_upsert(
    subs: &mut Vec<Subscriber>,
    client: ClientId,
    sender: mpsc::Sender<ServerFrame>,
) {
    if let Some(existing) = subs.iter_mut().find(|s| s.client == client) {
        existing.sender = sender;
    } else {
        subs.push(Subscriber { client, sender });
    }
}

pub(crate) type SessionTable = HashMap<SessionId, LiveSession>;

#[cfg(test)]
mod tests {
    use super::{
        PromptAction, SessionInner, Subscriber, prompt_action, subscriber_map_remove,
        subscriber_upsert,
    };
    use goat_protocol::{AskQuestion, Event, TaskId, ToolCallId};
    use goat_wire::{ClientId, ServerFrame, SessionId, SessionLiveState};
    use tokio::sync::mpsc;

    fn blank_inner() -> SessionInner {
        let (ops, _ops_rx) = mpsc::channel(8);
        SessionInner {
            id: SessionId(1),
            cwd: "/tmp".to_owned(),
            created_at: 0,
            ops,
            log: std::collections::VecDeque::new(),
            next_seq: 0,
            next_task: 1,
            subscribers: Vec::new(),
            state: SessionLiveState::Idle {},
            snapshot: None,
            tokens: 0,
            open_asks: 0,
            thread_id: None,
            awaits_restore: false,
            ready: std::sync::Arc::new(tokio::sync::Notify::new()),
            resurrected: std::collections::HashSet::new(),
        }
    }

    #[test]
    fn upsert_replaces_sender_for_same_client() {
        let mut subs: Vec<Subscriber> = Vec::new();
        let (a, _ra) = mpsc::channel::<ServerFrame>(8);
        let (b, _rb) = mpsc::channel::<ServerFrame>(8);
        subscriber_upsert(&mut subs, ClientId(7), a);
        subscriber_upsert(&mut subs, ClientId(7), b);
        assert_eq!(subs.len(), 1);
        subscriber_map_remove(&mut subs, ClientId(7));
        assert!(subs.is_empty());
    }

    #[test]
    fn restored_watermark_skips_its_own_event() {
        let mut inner = blank_inner();
        inner.thread_id = Some(1);
        let event = Event::ConversationRestored {
            target: goat_protocol::ModelTarget {
                provider: "p".to_owned(),
                model: "m".to_owned(),
                account: "a".to_owned(),
                effort: None,
            },
            entries: Vec::new(),
            context_tokens: None,
            compaction_threshold: None,
        };
        inner.record_and_fanout(event);
        let snap = inner.snapshot.clone().expect("snapshot recorded");
        let restored_seq = inner.log.back().map(|(seq, _)| *seq).unwrap();
        assert!(
            restored_seq < snap.watermark,
            "ConversationRestored seq {restored_seq} must be below watermark {}",
            snap.watermark
        );
    }

    #[test]
    fn log_is_bounded() {
        let mut inner = blank_inner();
        for _ in 0..(super::MAX_RETAINED_EVENTS + 50) {
            inner.record_and_fanout(Event::TextDelta {
                id: TaskId(0),
                chunk: "x".to_owned(),
            });
        }
        assert_eq!(inner.log.len(), super::MAX_RETAINED_EVENTS);
    }

    #[test]
    fn ask_started_maps_to_open_prompt() {
        let event = Event::AskStarted {
            id: TaskId(5),
            call: ToolCallId(9),
            questions: vec![AskQuestion {
                question: "Deploy?".to_owned(),
                options: Vec::new(),
                multiple: false,
            }],
        };
        match prompt_action(&event) {
            Some(PromptAction::Open {
                call_id,
                kind,
                task_id,
                ..
            }) => {
                assert_eq!(call_id, "9");
                assert_eq!(kind, "ask");
                assert_eq!(task_id, 5);
            }
            _ => panic!("expected open prompt"),
        }
    }

    #[test]
    fn ask_dismissed_maps_to_close() {
        let event = Event::AskDismissed {
            id: TaskId(5),
            call: ToolCallId(9),
        };
        match prompt_action(&event) {
            Some(PromptAction::Close { call_id }) => assert_eq!(call_id, "9"),
            _ => panic!("expected close prompt"),
        }
    }
}
