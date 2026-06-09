use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::{Connection, OptionalExtension, params};

const LATEST_VERSION: i64 = 2;

const SCHEMA_V1: &str = "\
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

const SCHEMA_V2: &str = "\
ALTER TABLE threads ADD COLUMN effort TEXT;
ALTER TABLE turns ADD COLUMN effort TEXT;";

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let mut version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > LATEST_VERSION {
        return Err(StoreError::UnknownVersion(version));
    }
    while version < LATEST_VERSION {
        match version {
            0 => conn.execute_batch(SCHEMA_V1)?,
            1 => conn.execute_batch(SCHEMA_V2)?,
            _ => return Err(StoreError::UnknownVersion(version)),
        }
        version += 1;
        conn.execute_batch(&format!("PRAGMA user_version = {version};"))?;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database version {0} is newer than this binary supports")]
    UnknownVersion(i64),
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
    pub turn_id: Option<i64>,
    pub role: String,
    pub body: String,
    pub created_at: i64,
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

fn thread_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Thread> {
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

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        migrate(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        migrate(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    async fn run<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let guard = conn.lock().expect("store mutex poisoned");
            f(&guard)
        })
        .await
        .expect("store blocking task panicked")
    }

    pub async fn create_thread(&self, thread: NewThread) -> Result<i64, StoreError> {
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO threads (cwd, title, provider, model, account, effort, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    thread.cwd,
                    thread.title,
                    thread.provider,
                    thread.model,
                    thread.account,
                    thread.effort,
                    thread.created_at,
                    thread.updated_at,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    pub async fn get_thread(&self, id: i64) -> Result<Option<Thread>, StoreError> {
        self.run(move |conn| {
            conn.query_row(
                "SELECT id, cwd, title, provider, model, account, effort, created_at, updated_at
                 FROM threads WHERE id = ?1",
                params![id],
                thread_from_row,
            )
            .optional()
            .map_err(StoreError::from)
        })
        .await
    }

    pub async fn latest_thread_in(&self, cwd: String) -> Result<Option<Thread>, StoreError> {
        self.run(move |conn| {
            conn.query_row(
                "SELECT id, cwd, title, provider, model, account, effort, created_at, updated_at
                 FROM threads WHERE cwd = ?1 ORDER BY updated_at DESC, id DESC LIMIT 1",
                params![cwd],
                thread_from_row,
            )
            .optional()
            .map_err(StoreError::from)
        })
        .await
    }

    pub async fn list_threads_in(
        &self,
        cwd: String,
        limit: i64,
    ) -> Result<Vec<Thread>, StoreError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, cwd, title, provider, model, account, effort, created_at, updated_at
                 FROM threads WHERE cwd = ?1 ORDER BY updated_at DESC, id DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![cwd, limit], thread_from_row)?;
            let mut threads = Vec::new();
            for row in rows {
                threads.push(row?);
            }
            Ok(threads)
        })
        .await
    }

    pub async fn get_messages(&self, thread_id: i64) -> Result<Vec<StoredMessage>, StoreError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT turn_id, role, body, created_at
                 FROM messages WHERE thread_id = ?1 ORDER BY id ASC",
            )?;
            let rows = stmt.query_map(params![thread_id], |row| {
                Ok(StoredMessage {
                    turn_id: row.get(0)?,
                    role: row.get(1)?,
                    body: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })?;
            let mut messages = Vec::new();
            for row in rows {
                messages.push(row?);
            }
            Ok(messages)
        })
        .await
    }

    pub async fn update_thread_model(
        &self,
        id: i64,
        provider: String,
        model: String,
        account: String,
        effort: Option<String>,
        updated_at: i64,
    ) -> Result<(), StoreError> {
        self.run(move |conn| {
            conn.execute(
                "UPDATE threads SET provider = ?2, model = ?3, account = ?4, effort = ?5, updated_at = ?6
                 WHERE id = ?1",
                params![id, provider, model, account, effort, updated_at],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn update_thread_title(&self, id: i64, title: String) -> Result<(), StoreError> {
        self.run(move |conn| {
            conn.execute(
                "UPDATE threads SET title = ?2 WHERE id = ?1",
                params![id, title],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn create_turn(&self, turn: NewTurn) -> Result<i64, StoreError> {
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO turns (thread_id, task_id, provider, model, account, effort, status, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    turn.thread_id,
                    turn.task_id,
                    turn.provider,
                    turn.model,
                    turn.account,
                    turn.effort,
                    turn.status,
                    turn.started_at,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    pub async fn finish_turn(
        &self,
        id: i64,
        status: String,
        finished_at: i64,
    ) -> Result<(), StoreError> {
        self.run(move |conn| {
            conn.execute(
                "UPDATE turns SET status = ?2, finished_at = ?3 WHERE id = ?1",
                params![id, status, finished_at],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn create_message(&self, message: NewMessage) -> Result<i64, StoreError> {
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO messages (thread_id, turn_id, role, body, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    message.thread_id,
                    message.turn_id,
                    message.role,
                    message.body,
                    message.created_at,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    pub async fn create_tool_call(&self, call: NewToolCall) -> Result<i64, StoreError> {
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO tool_calls (thread_id, turn_id, call_id, name, input, status, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    call.thread_id,
                    call.turn_id,
                    call.call_id,
                    call.name,
                    call.input,
                    call.status,
                    call.started_at,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    pub async fn finish_tool_call(
        &self,
        id: i64,
        status: String,
        summary: Option<String>,
        finished_at: i64,
    ) -> Result<(), StoreError> {
        self.run(move |conn| {
            conn.execute(
                "UPDATE tool_calls SET status = ?2, summary = ?3, finished_at = ?4 WHERE id = ?1",
                params![id, status, summary, finished_at],
            )?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::{NewMessage, NewThread, NewToolCall, NewTurn, Store};

    fn sample_thread() -> NewThread {
        NewThread {
            cwd: "/tmp/project".into(),
            title: Some("first".into()),
            provider: "openai".into(),
            model: "gpt-x".into(),
            account: "default".into(),
            effort: None,
            created_at: 100,
            updated_at: 100,
        }
    }

    #[tokio::test]
    async fn migrates_and_roundtrips_thread() {
        let store = Store::open_in_memory().unwrap();
        let id = store.create_thread(sample_thread()).await.unwrap();
        let thread = store.get_thread(id).await.unwrap().unwrap();
        assert_eq!(thread.provider, "openai");
        assert_eq!(thread.model, "gpt-x");
        assert_eq!(thread.title.as_deref(), Some("first"));
    }

    #[tokio::test]
    async fn latest_thread_in_is_scoped_to_cwd_and_recency() {
        let store = Store::open_in_memory().unwrap();
        let make = |cwd: &str, model: &str, updated: i64| NewThread {
            cwd: cwd.into(),
            title: None,
            provider: "openai".into(),
            model: model.into(),
            account: "default".into(),
            effort: None,
            created_at: updated,
            updated_at: updated,
        };
        store.create_thread(make("/a", "old", 100)).await.unwrap();
        store.create_thread(make("/a", "new", 200)).await.unwrap();
        store.create_thread(make("/b", "other", 300)).await.unwrap();

        let latest = store.latest_thread_in("/a".into()).await.unwrap().unwrap();
        assert_eq!(latest.model, "new");
        assert!(
            store
                .latest_thread_in("/missing".into())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn updates_thread_model_snapshot() {
        let store = Store::open_in_memory().unwrap();
        let id = store.create_thread(sample_thread()).await.unwrap();
        store
            .update_thread_model(
                id,
                "anthropic".into(),
                "claude".into(),
                "work".into(),
                Some("high".into()),
                200,
            )
            .await
            .unwrap();
        let thread = store.get_thread(id).await.unwrap().unwrap();
        assert_eq!(thread.provider, "anthropic");
        assert_eq!(thread.model, "claude");
        assert_eq!(thread.account, "work");
        assert_eq!(thread.effort.as_deref(), Some("high"));
        assert_eq!(thread.updated_at, 200);
    }

    #[tokio::test]
    async fn persists_turn_message_and_tool_call() {
        let store = Store::open_in_memory().unwrap();
        let thread_id = store.create_thread(sample_thread()).await.unwrap();
        let turn_id = store
            .create_turn(NewTurn {
                thread_id,
                task_id: 1,
                provider: "openai".into(),
                model: "gpt-x".into(),
                account: "default".into(),
                effort: None,
                status: "running".into(),
                started_at: 110,
            })
            .await
            .unwrap();
        store
            .create_message(NewMessage {
                thread_id,
                turn_id: Some(turn_id),
                role: "user".into(),
                body: "hello".into(),
                created_at: 111,
            })
            .await
            .unwrap();
        let call_id = store
            .create_tool_call(NewToolCall {
                thread_id,
                turn_id,
                call_id: "call-1".into(),
                name: "Read".into(),
                input: "file.rs".into(),
                status: "running".into(),
                started_at: 112,
            })
            .await
            .unwrap();
        store
            .finish_tool_call(call_id, "done".into(), Some("ok".into()), 113)
            .await
            .unwrap();
        store
            .finish_turn(turn_id, "done".into(), 120)
            .await
            .unwrap();

        let thread = store.get_thread(thread_id).await.unwrap().unwrap();
        assert_eq!(thread.provider, "openai");
    }

    #[tokio::test]
    async fn lists_threads_and_reads_messages_in_order() {
        let store = Store::open_in_memory().unwrap();
        let make = |model: &str, updated: i64| NewThread {
            cwd: "/proj".into(),
            title: Some(format!("thread {model}")),
            provider: "openai".into(),
            model: model.into(),
            account: "default".into(),
            effort: Some("high".into()),
            created_at: updated,
            updated_at: updated,
        };
        let first = store.create_thread(make("a", 100)).await.unwrap();
        let second = store.create_thread(make("b", 200)).await.unwrap();

        let threads = store.list_threads_in("/proj".into(), 10).await.unwrap();
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].id, second);
        assert_eq!(threads[0].effort.as_deref(), Some("high"));
        assert_eq!(threads[1].id, first);

        for (idx, body) in ["[{\"type\":\"text\",\"text\":\"hi\"}]", "second"]
            .into_iter()
            .enumerate()
        {
            store
                .create_message(NewMessage {
                    thread_id: first,
                    turn_id: None,
                    role: "user".into(),
                    body: body.into(),
                    created_at: 110 + i64::try_from(idx).unwrap(),
                })
                .await
                .unwrap();
        }
        let messages = store.get_messages(first).await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].body, "[{\"type\":\"text\",\"text\":\"hi\"}]");
        assert_eq!(messages[1].body, "second");
    }

    #[tokio::test]
    async fn reopen_file_is_idempotent() {
        let path = std::env::temp_dir().join("goat-store-reopen-test.db");
        let _ = std::fs::remove_file(&path);
        let id;
        {
            let store = Store::open(&path).unwrap();
            id = store.create_thread(sample_thread()).await.unwrap();
        }
        let store = Store::open(&path).unwrap();
        let thread = store.get_thread(id).await.unwrap();
        assert!(thread.is_some());
        let _ = std::fs::remove_file(&path);
    }
}
