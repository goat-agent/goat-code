use rusqlite::Connection;

use crate::StoreError;

pub(crate) const LATEST_VERSION: i64 = 6;

pub(crate) const SCHEMA_V1: &str = "\
CREATE TABLE threads (
    id INTEGER PRIMARY KEY,
    cwd TEXT NOT NULL,
    title TEXT,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    account TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE turns (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL,
    task_id INTEGER NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    account TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE TABLE messages (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL,
    turn_id INTEGER,
    role TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE tool_calls (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL,
    turn_id INTEGER NOT NULL,
    call_id TEXT NOT NULL,
    name TEXT NOT NULL,
    input TEXT NOT NULL,
    status TEXT NOT NULL,
    summary TEXT,
    started_at INTEGER NOT NULL,
    finished_at INTEGER
);";

pub(crate) const SCHEMA_V2: &str = "\
ALTER TABLE threads ADD COLUMN effort TEXT;
ALTER TABLE turns ADD COLUMN effort TEXT;";

pub(crate) const SCHEMA_V3: &str = "\
CREATE INDEX idx_messages_thread ON messages(thread_id);
CREATE INDEX idx_tool_calls_thread ON tool_calls(thread_id);
CREATE INDEX idx_threads_cwd ON threads(cwd);";

pub(crate) const SCHEMA_V4: &str = "\
CREATE TABLE compactions (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL,
    summary TEXT NOT NULL,
    after_message_id INTEGER NOT NULL,
    tail_from_message_id INTEGER,
    preserved_message_ids TEXT NOT NULL,
    tokens_before INTEGER NOT NULL,
    tokens_after INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_compactions_thread ON compactions(thread_id);";

pub(crate) const SCHEMA_V5: &str = "ALTER TABLE threads ADD COLUMN mode TEXT;";

pub(crate) const SCHEMA_V6: &str = "\
CREATE TABLE session_events (
    thread_id INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (thread_id, seq)
);
CREATE TABLE open_prompts (
    thread_id INTEGER NOT NULL,
    call_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    task_id INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (thread_id, call_id)
);";

pub(crate) fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let mut version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > LATEST_VERSION {
        return Err(StoreError::UnknownVersion(version));
    }
    while version < LATEST_VERSION {
        match version {
            0 => conn.execute_batch(SCHEMA_V1)?,
            1 => conn.execute_batch(SCHEMA_V2)?,
            2 => conn.execute_batch(SCHEMA_V3)?,
            3 => conn.execute_batch(SCHEMA_V4)?,
            4 => conn.execute_batch(SCHEMA_V5)?,
            5 => conn.execute_batch(SCHEMA_V6)?,
            _ => return Err(StoreError::UnknownVersion(version)),
        }
        version += 1;
        conn.execute_batch(&format!("PRAGMA user_version = {version};"))?;
    }
    Ok(())
}
