use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TaskId(pub u64);

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ToolCallId(pub u64);

impl fmt::Display for ToolCallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    pub input: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub ok: bool,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Recoverable,
    Fatal,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelTarget {
    pub provider: String,
    pub model: String,
    pub account: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountChoice {
    pub id: String,
    pub display: String,
    pub target: ModelTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub provider: String,
    pub model: String,
    pub accounts: Vec<AccountChoice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    None,
    ApiKey,
    OAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginProvider {
    pub id: String,
    pub method: AuthMethod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoginCredential {
    ApiKey(String),
    OAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    SubmitMessage {
        id: TaskId,
        text: String,
    },
    Interrupt {
        id: TaskId,
    },
    SelectModel {
        target: ModelTarget,
    },
    RefreshModels,
    Login {
        provider: String,
        credential: LoginCredential,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    TaskStarted {
        id: TaskId,
    },
    TextDelta {
        id: TaskId,
        chunk: String,
    },
    TextDone {
        id: TaskId,
        text: String,
    },
    ToolStarted {
        id: TaskId,
        call: ToolCall,
    },
    ToolDone {
        id: TaskId,
        call: ToolCallId,
        outcome: ToolOutcome,
    },
    TaskDone {
        id: TaskId,
        interrupted: bool,
    },
    ModelListChanged {
        entries: Vec<ModelEntry>,
    },
    ModelSelected {
        target: ModelTarget,
    },
    LoginProviders {
        providers: Vec<LoginProvider>,
    },
    LoginStatus {
        provider: String,
        message: String,
        done: bool,
        ok: bool,
    },
    Error {
        id: Option<TaskId>,
        severity: Severity,
        message: String,
    },
}
