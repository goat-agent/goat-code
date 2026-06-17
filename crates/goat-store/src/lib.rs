mod models;
mod schema;

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::{Connection, OptionalExtension, params};

use models::thread_from_row;
use schema::migrate;

pub use models::{
    Compaction, NewCompaction, NewMessage, NewThread, NewToolCall, NewTurn, OpenPrompt, StoreError,
    StoredMessage, Thread,
};

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
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
        match tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            f(&guard)
        })
        .await
        {
            Ok(result) => result,
            Err(err) => Err(StoreError::BlockingTask(err.to_string())),
        }
    }

    pub async fn create_thread(&self, thread: NewThread) -> Result<i64, StoreError> {
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO threads (cwd, title, provider, model, account, effort, mode, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    thread.cwd,
                    thread.title,
                    thread.provider,
                    thread.model,
                    thread.account,
                    thread.effort,
                    thread.mode,
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
                "SELECT id, cwd, title, provider, model, account, effort, mode, created_at, updated_at
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
                "SELECT id, cwd, title, provider, model, account, effort, mode, created_at, updated_at
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
                "SELECT id, cwd, title, provider, model, account, effort, mode, created_at, updated_at
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

    pub async fn last_turn_interrupted(&self, thread_id: i64) -> Result<bool, StoreError> {
        self.run(move |conn| {
            let status: Option<String> = conn
                .query_row(
                    "SELECT status FROM turns WHERE thread_id = ?1
                     ORDER BY id DESC LIMIT 1",
                    params![thread_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(matches!(status.as_deref(), Some("interrupted")))
        })
        .await
    }

    pub async fn append_session_event(
        &self,
        thread_id: i64,
        body: String,
        created_at: i64,
    ) -> Result<(), StoreError> {
        self.run(move |conn| {
            let next: i64 = conn.query_row(
                "SELECT COALESCE(MAX(seq), -1) + 1 FROM session_events WHERE thread_id = ?1",
                params![thread_id],
                |row| row.get(0),
            )?;
            conn.execute(
                "INSERT INTO session_events (thread_id, seq, body, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![thread_id, next, body, created_at],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn session_events(&self, thread_id: i64) -> Result<Vec<(u64, String)>, StoreError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT seq, body FROM session_events
                 WHERE thread_id = ?1 ORDER BY seq ASC",
            )?;
            let rows = stmt.query_map(params![thread_id], |row| {
                let seq: i64 = row.get(0)?;
                let body: String = row.get(1)?;
                Ok((u64::try_from(seq).unwrap_or(0), body))
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .await
    }

    pub async fn record_open_prompt(
        &self,
        thread_id: i64,
        call_id: String,
        kind: String,
        payload: String,
        task_id: u64,
        created_at: i64,
    ) -> Result<(), StoreError> {
        let task_id = i64::try_from(task_id).unwrap_or(i64::MAX);
        self.run(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO open_prompts
                 (thread_id, call_id, kind, payload, task_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![thread_id, call_id, kind, payload, task_id, created_at],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn clear_open_prompt(
        &self,
        thread_id: i64,
        call_id: String,
    ) -> Result<(), StoreError> {
        self.run(move |conn| {
            conn.execute(
                "DELETE FROM open_prompts WHERE thread_id = ?1 AND call_id = ?2",
                params![thread_id, call_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn open_prompts(&self, thread_id: i64) -> Result<Vec<OpenPrompt>, StoreError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT call_id, kind, payload, task_id FROM open_prompts
                 WHERE thread_id = ?1 ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map(params![thread_id], |row| {
                let task_id: i64 = row.get(3)?;
                Ok(OpenPrompt {
                    call_id: row.get(0)?,
                    kind: row.get(1)?,
                    payload: row.get(2)?,
                    task_id: u64::try_from(task_id).unwrap_or(0),
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .await
    }

    pub async fn get_messages(&self, thread_id: i64) -> Result<Vec<StoredMessage>, StoreError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, turn_id, role, body, created_at
                 FROM messages WHERE thread_id = ?1 ORDER BY id ASC",
            )?;
            let rows = stmt.query_map(params![thread_id], |row| {
                Ok(StoredMessage {
                    id: row.get(0)?,
                    turn_id: row.get(1)?,
                    role: row.get(2)?,
                    body: row.get(3)?,
                    created_at: row.get(4)?,
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

    pub async fn update_thread_mode(
        &self,
        id: i64,
        mode: Option<String>,
        updated_at: i64,
    ) -> Result<(), StoreError> {
        self.run(move |conn| {
            conn.execute(
                "UPDATE threads SET mode = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, mode, updated_at],
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

    pub async fn mark_running_turns_interrupted(
        &self,
        finished_at: i64,
    ) -> Result<usize, StoreError> {
        self.run(move |conn| {
            let changed = conn.execute(
                "UPDATE turns SET status = 'interrupted', finished_at = ?1
                 WHERE status = 'running'",
                params![finished_at],
            )?;
            Ok(changed)
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

    pub async fn create_compaction(&self, compaction: NewCompaction) -> Result<i64, StoreError> {
        self.run(move |conn| {
            let preserved = serde_json::to_string(&compaction.preserved_message_ids)
                .unwrap_or_else(|_| "[]".to_owned());
            conn.execute(
                "INSERT INTO compactions (thread_id, summary, after_message_id, tail_from_message_id, preserved_message_ids, tokens_before, tokens_after, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    compaction.thread_id,
                    compaction.summary,
                    compaction.after_message_id,
                    compaction.tail_from_message_id,
                    preserved,
                    compaction.tokens_before,
                    compaction.tokens_after,
                    compaction.created_at,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    pub async fn compactions_for_thread(
        &self,
        thread_id: i64,
    ) -> Result<Vec<Compaction>, StoreError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, thread_id, summary, after_message_id, tail_from_message_id, preserved_message_ids, tokens_before, tokens_after, created_at
                 FROM compactions WHERE thread_id = ?1 ORDER BY id ASC",
            )?;
            let rows = stmt.query_map(params![thread_id], |row| {
                let preserved_raw: String = row.get(5)?;
                Ok(Compaction {
                    id: row.get(0)?,
                    thread_id: row.get(1)?,
                    summary: row.get(2)?,
                    after_message_id: row.get(3)?,
                    tail_from_message_id: row.get(4)?,
                    preserved_message_ids: serde_json::from_str(&preserved_raw)
                        .unwrap_or_default(),
                    tokens_before: row.get(6)?,
                    tokens_after: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })?;
            let mut compactions = Vec::new();
            for row in rows {
                compactions.push(row?);
            }
            Ok(compactions)
        })
        .await
    }
}

#[cfg(test)]
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
            mode: None,
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
        assert_eq!(thread.mode, None);
    }

    #[tokio::test]
    async fn updates_thread_mode() {
        let store = Store::open_in_memory().unwrap();
        let id = store.create_thread(sample_thread()).await.unwrap();
        store
            .update_thread_mode(id, Some("plan".into()), 200)
            .await
            .unwrap();
        let thread = store.get_thread(id).await.unwrap().unwrap();
        assert_eq!(thread.mode.as_deref(), Some("plan"));
        store.update_thread_mode(id, None, 300).await.unwrap();
        let thread = store.get_thread(id).await.unwrap().unwrap();
        assert_eq!(thread.mode, None);
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
            mode: None,
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
    async fn session_events_append_and_read_in_order() {
        let store = Store::open_in_memory().unwrap();
        let thread_id = store.create_thread(sample_thread()).await.unwrap();
        store
            .append_session_event(thread_id, "a".into(), 1)
            .await
            .unwrap();
        store
            .append_session_event(thread_id, "b".into(), 2)
            .await
            .unwrap();
        let events = store.session_events(thread_id).await.unwrap();
        assert_eq!(events, vec![(0, "a".to_owned()), (1, "b".to_owned())]);
    }

    #[tokio::test]
    async fn open_prompts_roundtrip_and_clear() {
        let store = Store::open_in_memory().unwrap();
        let thread_id = store.create_thread(sample_thread()).await.unwrap();
        store
            .record_open_prompt(thread_id, "7".into(), "ask".into(), "[]".into(), 3, 100)
            .await
            .unwrap();
        let prompts = store.open_prompts(thread_id).await.unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].call_id, "7");
        assert_eq!(prompts[0].kind, "ask");
        assert_eq!(prompts[0].task_id, 3);

        store
            .clear_open_prompt(thread_id, "7".into())
            .await
            .unwrap();
        assert!(store.open_prompts(thread_id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sweep_marks_only_running_turns_interrupted() {
        let store = Store::open_in_memory().unwrap();
        let thread_id = store.create_thread(sample_thread()).await.unwrap();
        let make_turn = |task_id, status: &str| NewTurn {
            thread_id,
            task_id,
            provider: "openai".into(),
            model: "gpt-x".into(),
            account: "default".into(),
            effort: None,
            status: status.into(),
            started_at: 100,
        };
        let running = store.create_turn(make_turn(1, "running")).await.unwrap();
        let done = store.create_turn(make_turn(2, "done")).await.unwrap();
        store.finish_turn(done, "done".into(), 120).await.unwrap();

        let changed = store.mark_running_turns_interrupted(200).await.unwrap();
        assert_eq!(changed, 1);

        let again = store.mark_running_turns_interrupted(300).await.unwrap();
        assert_eq!(again, 0, "second sweep is a no-op");
        let _ = (running, done);
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
            mode: None,
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
    async fn compaction_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let thread_id = store.create_thread(sample_thread()).await.unwrap();
        let id = store
            .create_compaction(super::NewCompaction {
                thread_id,
                summary: "## Task\nbuild the thing".into(),
                after_message_id: 42,
                tail_from_message_id: Some(40),
                preserved_message_ids: vec![38, 41],
                tokens_before: 170_000,
                tokens_after: 24_000,
                created_at: 500,
            })
            .await
            .unwrap();
        let compactions = store.compactions_for_thread(thread_id).await.unwrap();
        assert_eq!(compactions.len(), 1);
        assert_eq!(compactions[0].id, id);
        assert_eq!(compactions[0].after_message_id, 42);
        assert_eq!(compactions[0].tail_from_message_id, Some(40));
        assert_eq!(compactions[0].preserved_message_ids, vec![38, 41]);
        assert_eq!(compactions[0].tokens_before, 170_000);
        assert!(
            store
                .compactions_for_thread(thread_id + 1)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn stored_messages_carry_row_ids() {
        let store = Store::open_in_memory().unwrap();
        let thread_id = store.create_thread(sample_thread()).await.unwrap();
        let first = store
            .create_message(NewMessage {
                thread_id,
                turn_id: None,
                role: "user".into(),
                body: "one".into(),
                created_at: 1,
            })
            .await
            .unwrap();
        let second = store
            .create_message(NewMessage {
                thread_id,
                turn_id: None,
                role: "assistant".into(),
                body: "two".into(),
                created_at: 2,
            })
            .await
            .unwrap();
        let messages = store.get_messages(thread_id).await.unwrap();
        assert_eq!(messages[0].id, first);
        assert_eq!(messages[1].id, second);
    }

    #[tokio::test]
    async fn migrates_v3_database_to_v4() {
        let path = std::env::temp_dir().join("goat-store-v3-migration-test.db");
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(crate::schema::SCHEMA_V1).unwrap();
            conn.execute_batch(crate::schema::SCHEMA_V2).unwrap();
            conn.execute_batch(crate::schema::SCHEMA_V3).unwrap();
            conn.execute_batch("PRAGMA user_version = 3;").unwrap();
        }
        let store = Store::open(&path).unwrap();
        let thread_id = store.create_thread(sample_thread()).await.unwrap();
        store
            .create_compaction(super::NewCompaction {
                thread_id,
                summary: "s".into(),
                after_message_id: 1,
                tail_from_message_id: None,
                preserved_message_ids: vec![],
                tokens_before: 10,
                tokens_after: 5,
                created_at: 1,
            })
            .await
            .unwrap();
        assert_eq!(
            store.compactions_for_thread(thread_id).await.unwrap().len(),
            1
        );
        let _ = std::fs::remove_file(&path);
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

    #[tokio::test]
    async fn open_sets_wal_and_busy_timeout() {
        let path = std::env::temp_dir().join("goat-store-pragma-test.db");
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path).unwrap();
        let (mode, timeout) = store
            .run(|conn| {
                let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
                let timeout: i64 = conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
                Ok((mode, timeout))
            })
            .await
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        assert!(timeout >= 5000);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }
}
