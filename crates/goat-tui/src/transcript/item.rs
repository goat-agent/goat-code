use goat_protocol::{TaskId, ToolCallId, ToolDisplay, ToolOutcome};

pub(crate) struct Working {
    pub elapsed: Option<u64>,
    pub label: Option<String>,
    pub thinking: bool,
    pub tokens: Option<u64>,
}

#[derive(Debug)]
pub(crate) enum ToolStatus {
    Running,
    Done(ToolOutcome),
}

#[derive(Debug)]
pub(crate) enum ShellStatus {
    Running,
    Done(String),
}

#[derive(Debug)]
pub(crate) enum Item {
    User(String),
    Agent(String),
    Tool {
        id: ToolCallId,
        name: String,
        display: ToolDisplay,
        status: ToolStatus,
        image: Option<Box<crate::screenshot::TranscriptImage>>,
    },
    Shell {
        id: TaskId,
        command: String,
        status: ShellStatus,
    },
    Error(String),
    Notice(String),
    Compaction {
        tokens_before: u32,
        tokens_after: u32,
    },
}
