use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateWindow {
    pub label: String,
    pub used_percent: f32,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimitSnapshot {
    pub windows: Vec<RateWindow>,
    #[serde(default)]
    pub representative: Option<String>,
}

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
pub struct ToolDisplay {
    pub primary: String,
    pub detail: Option<String>,
}

impl ToolDisplay {
    pub fn primary(primary: impl Into<String>) -> Self {
        Self {
            primary: primary.into(),
            detail: None,
        }
    }

    pub fn with_detail(primary: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            primary: primary.into(),
            detail: Some(detail.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    pub display: ToolDisplay,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub ok: bool,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Off,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }
}

impl fmt::Display for Effort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Normal,
    Plan,
}

impl Mode {
    #[must_use]
    pub fn is_plan(self) -> bool {
        matches!(self, Self::Plan)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanDecision {
    Approve,
    Reject { feedback: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelTarget {
    pub provider: String,
    pub model: String,
    pub account: String,
    #[serde(default)]
    pub effort: Option<Effort>,
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
    pub context_window: Option<u32>,
    #[serde(default)]
    pub efforts: Vec<Effort>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub id: i64,
    pub title: String,
    pub model: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TranscriptEntry {
    User(String),
    Assistant(String),
    Tool {
        call: ToolCall,
        outcome: ToolOutcome,
    },
    Compaction {
        tokens_before: u32,
        tokens_after: u32,
    },
    Shell {
        command: String,
        output: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    None,
    ApiKey,
    OAuth,
    ApiKeyOrOAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginProvider {
    pub id: String,
    pub method: AuthMethod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    pub name: String,
    pub method: AuthMethod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountEntry {
    pub provider: String,
    pub display_name: String,
    pub accounts: Vec<AccountInfo>,
    pub local: bool,
    pub login: AuthMethod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoginCredential {
    ApiKey(String),
    OAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    SubmitMessage {
        id: TaskId,
        text: String,
    },
    SubmitShell {
        id: TaskId,
        command: String,
    },
    Interrupt {
        id: TaskId,
    },
    Clear,
    SelectModel {
        target: ModelTarget,
    },
    Login {
        provider: String,
        credential: LoginCredential,
    },
    AddAccount {
        provider: String,
        name: String,
        credential: LoginCredential,
    },
    RemoveAccount {
        provider: String,
        name: String,
    },
    ListThreads,
    Resume {
        thread_id: i64,
    },
    RenameThread {
        title: String,
    },
    Answer {
        id: TaskId,
        call: ToolCallId,
        answers: Vec<String>,
    },
    Compact {
        id: TaskId,
        instructions: Option<String>,
    },
    DequeueMessage {
        id: TaskId,
    },
    SetMode {
        mode: Mode,
    },
    ResolvePlan {
        id: TaskId,
        call: ToolCallId,
        decision: PlanDecision,
    },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyKind {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    ShellDone {
        id: TaskId,
        output: String,
    },
    TaskDone {
        id: TaskId,
        interrupted: bool,
    },
    AgentStarted {
        id: TaskId,
        parent: TaskId,
        agent_type: String,
        label: String,
    },
    AgentDone {
        id: TaskId,
        ok: bool,
    },
    ModelListChanged {
        entries: Vec<ModelEntry>,
    },
    ModelSelected {
        target: ModelTarget,
    },
    ThreadsListed {
        threads: Vec<ThreadSummary>,
    },
    ConversationRestored {
        target: ModelTarget,
        entries: Vec<TranscriptEntry>,
        context_tokens: Option<u32>,
        compaction_threshold: Option<u32>,
        #[serde(default)]
        mode: Mode,
    },
    ThinkingDelta {
        id: TaskId,
        chunk: String,
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
    AccountsChanged {
        providers: Vec<AccountEntry>,
    },
    SkillsChanged {
        skills: Vec<SkillInfo>,
    },
    Error {
        id: Option<TaskId>,
        message: String,
    },
    Notify {
        kind: NotifyKind,
        message: String,
    },
    AskStarted {
        id: TaskId,
        call: ToolCallId,
        questions: Vec<AskQuestion>,
    },
    AskDismissed {
        id: TaskId,
        call: ToolCallId,
    },
    Usage {
        id: TaskId,
        usage: Usage,
        context_window: Option<u32>,
        compaction_threshold: Option<u32>,
    },
    RateLimits {
        provider: String,
        account: String,
        snapshot: RateLimitSnapshot,
        cached_at: i64,
    },
    Retrying {
        id: TaskId,
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        reason: String,
    },
    UserMessage {
        id: TaskId,
        text: String,
    },
    MessageDequeued {
        id: TaskId,
        text: String,
    },
    CompactionStarted {
        id: TaskId,
    },
    CompactionDone {
        id: TaskId,
        ok: bool,
        tokens_before: u32,
        tokens_after: u32,
        usage: Usage,
    },
    ModeChanged {
        mode: Mode,
        plan_path: Option<String>,
    },
    PlanProposed {
        id: TaskId,
        call: ToolCallId,
        plan: String,
        path: String,
    },
    PlanDismissed {
        id: TaskId,
        call: ToolCallId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestion {
    pub question: String,
    #[serde(default)]
    pub options: Vec<AskOption>,
}
