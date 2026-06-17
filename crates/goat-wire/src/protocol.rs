use serde::{Deserialize, Serialize};

use goat_protocol::{Event, ModelTarget, Op, TranscriptEntry};

pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClientId(pub u64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientFrame {
    Hello {
        version: u32,
    },
    OpenSession {
        cwd: String,
        resume: ResumeMode,
    },
    Attach {
        session: SessionId,
    },
    Submit {
        session: SessionId,
        correlation: u64,
        op: Op,
    },
    Control {
        session: SessionId,
        op: Op,
    },
    ListSessions,
    KillSession {
        session: SessionId,
    },
    StopDaemon,
    Goodbye,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResumeMode {
    New,
    Latest,
    Thread { thread_id: i64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerFrame {
    Welcome {
        version: u32,
        client_id: ClientId,
    },
    SessionOpened {
        session: SessionId,
    },
    Snapshot {
        session: SessionId,
        watermark: u64,
        target: Option<ModelTarget>,
        entries: Vec<TranscriptEntry>,
        context_tokens: Option<u32>,
        compaction_threshold: Option<u32>,
        mode: goat_protocol::Mode,
    },
    Event {
        session: SessionId,
        seq: u64,
        event: Event,
    },
    SessionList {
        sessions: Vec<SessionInfo>,
    },
    CorrelationAssigned {
        session: SessionId,
        correlation: u64,
        task: goat_protocol::TaskId,
    },
    Presence {
        session: SessionId,
        clients: Vec<ClientId>,
    },
    Error {
        message: String,
    },
    VersionMismatch {
        daemon_version: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session: SessionId,
    pub cwd: String,
    pub state: SessionLiveState,
    pub windows: usize,
    pub age_ms: i64,
    pub tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionLiveState {
    Idle,
    Active,
    WaitingOnAsk,
}
