#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database version {0} is newer than this binary supports")]
    UnknownVersion(i64),
    #[error("store task failed: {0}")]
    BlockingTask(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewThread {
    pub cwd: String,
    pub title: Option<String>,
    pub provider: String,
    pub model: String,
    pub account: String,
    pub effort: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub id: i64,
    pub cwd: String,
    pub title: Option<String>,
    pub provider: String,
    pub model: String,
    pub account: String,
    pub effort: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTurn {
    pub thread_id: i64,
    pub task_id: i64,
    pub provider: String,
    pub model: String,
    pub account: String,
    pub effort: Option<String>,
    pub status: String,
    pub started_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    pub thread_id: i64,
    pub turn_id: Option<i64>,
    pub role: String,
    pub body: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMessage {
    pub id: i64,
    pub turn_id: Option<i64>,
    pub role: String,
    pub body: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCompaction {
    pub thread_id: i64,
    pub summary: String,
    pub after_message_id: i64,
    pub tail_from_message_id: Option<i64>,
    pub preserved_message_ids: Vec<i64>,
    pub tokens_before: i64,
    pub tokens_after: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    pub id: i64,
    pub thread_id: i64,
    pub summary: String,
    pub after_message_id: i64,
    pub tail_from_message_id: Option<i64>,
    pub preserved_message_ids: Vec<i64>,
    pub tokens_before: i64,
    pub tokens_after: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPrompt {
    pub call_id: String,
    pub kind: String,
    pub payload: String,
    pub task_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewToolCall {
    pub thread_id: i64,
    pub turn_id: i64,
    pub call_id: String,
    pub name: String,
    pub input: String,
    pub status: String,
    pub started_at: i64,
}

pub(crate) fn thread_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Thread> {
    Ok(Thread {
        id: row.get(0)?,
        cwd: row.get(1)?,
        title: row.get(2)?,
        provider: row.get(3)?,
        model: row.get(4)?,
        account: row.get(5)?,
        effort: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}
