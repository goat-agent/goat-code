use std::{collections::HashMap, fmt::Write as _, path::Path, process::Stdio, sync::Arc};

use goat_protocol::{Event, ProcessExitReason, ProcessId, ProcessInfo, ProcessState};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{Mutex, mpsc},
};

const RING_CAPACITY: usize = 2000;
const MAX_LIVE_PROCESSES: usize = 16;
const WATCH_FLOOD_LINES: usize = 500;

pub(crate) struct Wake;

struct Line {
    stream: Stream,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stream {
    Out,
    Err,
}

struct Entry {
    command: String,
    pgid: Option<i32>,
    db_id: Option<i64>,
    lines: std::collections::VecDeque<Line>,
    dropped: usize,
    read_cursor: usize,
    watch_cursor: usize,
    total: usize,
    state: ProcessState,
    exit_code: Option<i32>,
    exit_observed: bool,
    watched: bool,
    watch_flooded: bool,
    stdin: Option<tokio::process::ChildStdin>,
    kill_pending: bool,
}

impl Entry {
    fn info(&self, id: ProcessId) -> ProcessInfo {
        ProcessInfo {
            id,
            command: self.command.clone(),
            state: self.state,
            watched: self.watched,
            exit_code: self.exit_code,
        }
    }
}

struct Inner {
    entries: HashMap<ProcessId, Entry>,
    next_id: u64,
}

pub(crate) struct ProcessRegistry {
    inner: Mutex<Inner>,
    events: mpsc::Sender<Event>,
    wake_tx: mpsc::Sender<Wake>,
    store: Option<goat_store::Store>,
}

#[derive(Debug)]
pub(crate) enum SpawnError {
    TooMany,
    Spawn(String),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooMany => write!(
                f,
                "too many background processes are already running (limit {MAX_LIVE_PROCESSES}); stop one with ProcessKill first"
            ),
            Self::Spawn(msg) => write!(f, "failed to start process: {msg}"),
        }
    }
}

pub(crate) struct Started {
    pub(crate) id: ProcessId,
    pub(crate) pgid: Option<i32>,
}

impl ProcessRegistry {
    pub(crate) fn new(
        events: mpsc::Sender<Event>,
        wake_tx: mpsc::Sender<Wake>,
        store: Option<goat_store::Store>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                entries: HashMap::new(),
                next_id: 1,
            }),
            events,
            wake_tx,
            store,
        })
    }

    pub(crate) async fn spawn(
        self: &Arc<Self>,
        command: &str,
        cwd: &Path,
        watched: bool,
    ) -> Result<Started, SpawnError> {
        let id = {
            let inner = self.inner.lock().await;
            let live = inner
                .entries
                .values()
                .filter(|e| e.state == ProcessState::Running)
                .count();
            if live >= MAX_LIVE_PROCESSES {
                return Err(SpawnError::TooMany);
            }
            ProcessId(inner.next_id)
        };

        let mut builder = Command::new("sh");
        builder
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        builder.process_group(0);

        let mut child = builder
            .spawn()
            .map_err(|err| SpawnError::Spawn(err.to_string()))?;

        #[cfg(unix)]
        let pgid = child.id().and_then(|pid| i32::try_from(pid).ok());
        #[cfg(not(unix))]
        let pgid = None;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdin = child.stdin.take();

        {
            let mut inner = self.inner.lock().await;
            inner.next_id += 1;
            inner.entries.insert(
                id,
                Entry {
                    command: command.to_owned(),
                    pgid,
                    db_id: None,
                    lines: std::collections::VecDeque::new(),
                    dropped: 0,
                    read_cursor: 0,
                    watch_cursor: 0,
                    total: 0,
                    state: ProcessState::Running,
                    exit_code: None,
                    exit_observed: false,
                    watched,
                    watch_flooded: false,
                    stdin,
                    kill_pending: false,
                },
            );
        }

        let _ = self
            .events
            .send(Event::ProcessStarted {
                process: id,
                command: command.to_owned(),
                watched,
            })
            .await;
        self.broadcast_list().await;

        if let Some(pipe) = stdout {
            self.spawn_reader(id, pipe, Stream::Out);
        }
        if let Some(pipe) = stderr {
            self.spawn_reader(id, pipe, Stream::Err);
        }
        self.spawn_waiter(id, child);

        Ok(Started { id, pgid })
    }

    pub(crate) async fn set_db_id(&self, id: ProcessId, db_id: i64) {
        let mut inner = self.inner.lock().await;
        if let Some(entry) = inner.entries.get_mut(&id) {
            entry.db_id = Some(db_id);
        }
    }

    fn spawn_reader<R>(self: &Arc<Self>, id: ProcessId, pipe: R, stream: Stream)
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            let mut reader = BufReader::new(pipe).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                registry.append_line(id, stream, line).await;
            }
        });
    }

    fn spawn_waiter(self: &Arc<Self>, id: ProcessId, mut child: tokio::process::Child) {
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            let status = child.wait().await;
            let code = status.ok().and_then(|s| s.code());
            registry
                .mark_exited(id, code, ProcessExitReason::Natural)
                .await;
        });
    }

    async fn append_line(self: &Arc<Self>, id: ProcessId, stream: Stream, text: String) {
        let should_wake = {
            let mut inner = self.inner.lock().await;
            let Some(entry) = inner.entries.get_mut(&id) else {
                return;
            };
            if entry.lines.len() >= RING_CAPACITY {
                entry.lines.pop_front();
                entry.dropped += 1;
                if entry.read_cursor > 0 {
                    entry.read_cursor -= 1;
                }
                if entry.watch_cursor > 0 {
                    entry.watch_cursor -= 1;
                }
            }
            entry.lines.push_back(Line {
                stream,
                text: text.clone(),
            });
            entry.total += 1;
            if entry.watched && !entry.watch_flooded {
                let pending = entry.lines.len() - entry.watch_cursor;
                if pending > WATCH_FLOOD_LINES {
                    entry.watched = false;
                    entry.watch_flooded = true;
                }
            }
            entry.watched
        };
        let _ = self
            .events
            .send(Event::ProcessOutput {
                process: id,
                chunk: text,
            })
            .await;
        if should_wake {
            let _ = self.wake_tx.send(Wake).await;
        }
    }

    async fn mark_exited(
        self: &Arc<Self>,
        id: ProcessId,
        code: Option<i32>,
        natural: ProcessExitReason,
    ) {
        let (watched, reason, db_id) = {
            let mut inner = self.inner.lock().await;
            let Some(entry) = inner.entries.get_mut(&id) else {
                return;
            };
            if entry.state == ProcessState::Exited {
                return;
            }
            entry.state = ProcessState::Exited;
            entry.exit_code = code;
            let reason = if entry.kill_pending {
                ProcessExitReason::Killed
            } else {
                natural
            };
            (entry.watched, reason, entry.db_id)
        };
        if let (Some(store), Some(db_id)) = (self.store.as_ref(), db_id) {
            let _ = store.finish_process(db_id, now_ms()).await;
        }
        let _ = self
            .events
            .send(Event::ProcessExited {
                process: id,
                code,
                reason,
            })
            .await;
        self.broadcast_list().await;
        if watched {
            let _ = self.wake_tx.send(Wake).await;
        }
    }

    pub(crate) async fn read_new(&self, id: ProcessId) -> Option<ReadChunk> {
        let mut inner = self.inner.lock().await;
        let entry = inner.entries.get_mut(&id)?;
        let chunk = collect_from(entry, entry.read_cursor);
        entry.read_cursor = entry.lines.len();
        Some(ReadChunk {
            text: chunk,
            state: entry.state,
            exit_code: entry.exit_code,
        })
    }

    pub(crate) async fn take_pending_observations(&self) -> Vec<(ProcessId, Observation)> {
        let mut inner = self.inner.lock().await;
        let ids: Vec<ProcessId> = inner.entries.keys().copied().collect();
        let mut out = Vec::new();
        for id in ids {
            let Some(entry) = inner.entries.get_mut(&id) else {
                continue;
            };
            if !entry.watched {
                continue;
            }
            let has_new = entry.lines.len() > entry.watch_cursor;
            let exited_unseen = entry.state == ProcessState::Exited && !entry.exit_observed;
            if !has_new && !exited_unseen {
                continue;
            }
            let text = collect_from(entry, entry.watch_cursor);
            entry.watch_cursor = entry.lines.len();
            entry.exit_observed = true;
            out.push((
                id,
                Observation {
                    command: entry.command.clone(),
                    output: text,
                    state: entry.state,
                    exit_code: entry.exit_code,
                },
            ));
        }
        out.sort_by_key(|(id, _)| id.0);
        out
    }

    pub(crate) async fn write_stdin(&self, id: ProcessId, text: &str) -> Result<(), String> {
        let mut stdin = {
            let mut inner = self.inner.lock().await;
            let entry = inner
                .entries
                .get_mut(&id)
                .ok_or_else(|| format!("no process #{id}"))?;
            if entry.state == ProcessState::Exited {
                return Err(format!(
                    "process #{id} has exited; start it again with ProcessStart"
                ));
            }
            entry
                .stdin
                .take()
                .ok_or_else(|| format!("process #{id} does not accept input"))?
        };
        let write = async {
            stdin.write_all(text.as_bytes()).await?;
            stdin.flush().await
        };
        let result = write.await;
        let mut inner = self.inner.lock().await;
        if let Some(entry) = inner.entries.get_mut(&id) {
            entry.stdin = Some(stdin);
        }
        result.map_err(|err| format!("failed to write to process #{id}: {err}"))
    }

    pub(crate) async fn set_watch(&self, id: ProcessId, on: bool) -> Result<(), String> {
        {
            let mut inner = self.inner.lock().await;
            let entry = inner
                .entries
                .get_mut(&id)
                .ok_or_else(|| format!("no process #{id}"))?;
            entry.watched = on;
            if on {
                entry.watch_flooded = false;
                entry.watch_cursor = entry.lines.len();
            }
        }
        self.broadcast_list().await;
        Ok(())
    }

    pub(crate) async fn kill(&self, id: ProcessId) -> Result<(), String> {
        let pgid = {
            let mut inner = self.inner.lock().await;
            let entry = inner
                .entries
                .get_mut(&id)
                .ok_or_else(|| format!("no process #{id}"))?;
            if entry.state == ProcessState::Exited {
                return Ok(());
            }
            entry.kill_pending = true;
            entry.pgid
        };
        kill_group(pgid);
        Ok(())
    }

    pub(crate) async fn list(&self) -> Vec<ProcessInfo> {
        let inner = self.inner.lock().await;
        collect_infos(&inner)
    }

    async fn broadcast_list(&self) {
        let processes = {
            let inner = self.inner.lock().await;
            collect_infos(&inner)
        };
        let _ = self
            .events
            .send(Event::ProcessListChanged { processes })
            .await;
    }

    pub(crate) async fn shutdown_all(&self) {
        let pgids: Vec<Option<i32>> = {
            let inner = self.inner.lock().await;
            inner
                .entries
                .values()
                .filter(|e| e.state == ProcessState::Running)
                .map(|e| e.pgid)
                .collect()
        };
        for pgid in pgids {
            kill_group(pgid);
        }
    }
}

pub(crate) struct ReadChunk {
    pub(crate) text: String,
    pub(crate) state: ProcessState,
    pub(crate) exit_code: Option<i32>,
}

pub(crate) struct Observation {
    pub(crate) command: String,
    pub(crate) output: String,
    pub(crate) state: ProcessState,
    pub(crate) exit_code: Option<i32>,
}

fn collect_from(entry: &Entry, cursor: usize) -> String {
    let mut out = String::new();
    if cursor == 0 && entry.dropped > 0 {
        let _ = writeln!(out, "[{} earlier lines dropped]", entry.dropped);
    }
    for line in entry.lines.iter().skip(cursor) {
        if line.stream == Stream::Err {
            out.push_str("[err] ");
        }
        out.push_str(&line.text);
        out.push('\n');
    }
    out
}

fn collect_infos(inner: &Inner) -> Vec<ProcessInfo> {
    let mut infos: Vec<ProcessInfo> = inner
        .entries
        .iter()
        .map(|(id, entry)| entry.info(*id))
        .collect();
    infos.sort_by_key(|i| i.id.0);
    infos
}

fn kill_group(pgid: Option<i32>) {
    #[cfg(unix)]
    if let Some(pgid) = pgid {
        let _ = std::process::Command::new("kill")
            .arg("-KILL")
            .arg(format!("-{pgid}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    let _ = pgid;
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{ProcessRegistry, Wake};
    use goat_protocol::{Event, ProcessState};
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn harness() -> (
        std::sync::Arc<ProcessRegistry>,
        mpsc::Receiver<Event>,
        mpsc::Receiver<Wake>,
    ) {
        let (event_tx, event_rx) = mpsc::channel(256);
        let (wake_tx, wake_rx) = mpsc::channel(256);
        let registry = ProcessRegistry::new(event_tx, wake_tx, None);
        (registry, event_rx, wake_rx)
    }

    async fn wait_until_exited(registry: &ProcessRegistry, id: goat_protocol::ProcessId) {
        for _ in 0..200 {
            let list = registry.list().await;
            if list
                .iter()
                .any(|p| p.id == id && p.state == ProcessState::Exited)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("process did not exit in time");
    }

    #[tokio::test]
    async fn spawn_reads_output_and_exits() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry
            .spawn("echo hello; echo world", &cwd, false)
            .await
            .unwrap_or_else(|e| panic!("spawn failed: {e}"));
        wait_until_exited(&registry, started.id).await;
        let chunk = registry.read_new(started.id).await.unwrap();
        assert!(chunk.text.contains("hello"), "got: {}", chunk.text);
        assert!(chunk.text.contains("world"), "got: {}", chunk.text);
        assert_eq!(chunk.state, ProcessState::Exited);
        assert_eq!(chunk.exit_code, Some(0));
    }

    #[tokio::test]
    async fn read_new_is_cursor_based() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn("echo one", &cwd, false).await.unwrap();
        wait_until_exited(&registry, started.id).await;
        let first = registry.read_new(started.id).await.unwrap();
        assert!(first.text.contains("one"));
        let second = registry.read_new(started.id).await.unwrap();
        assert!(
            !second.text.contains("one"),
            "second read should be empty of old output"
        );
    }

    #[tokio::test]
    async fn stderr_is_tagged() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn("echo oops 1>&2", &cwd, false).await.unwrap();
        wait_until_exited(&registry, started.id).await;
        let chunk = registry.read_new(started.id).await.unwrap();
        assert!(chunk.text.contains("[err] oops"), "got: {}", chunk.text);
    }

    #[tokio::test]
    async fn watched_process_wakes_on_output() {
        let (registry, _events, mut wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn("echo ping", &cwd, true).await.unwrap();
        let _woke = tokio::time::timeout(Duration::from_secs(5), wake.recv())
            .await
            .expect("should wake")
            .expect("wake channel open");
        let obs = registry.take_pending_observations().await;
        assert!(
            obs.iter()
                .any(|(id, o)| *id == started.id && o.output.contains("ping")),
            "got: {obs:?}",
            obs = obs
                .iter()
                .map(|(_, o)| o.output.clone())
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn unwatched_process_does_not_wake() {
        let (registry, _events, mut wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn("echo quiet", &cwd, false).await.unwrap();
        wait_until_exited(&registry, started.id).await;
        let result = tokio::time::timeout(Duration::from_millis(200), wake.recv()).await;
        assert!(result.is_err(), "unwatched process must not wake the agent");
    }

    #[tokio::test]
    async fn kill_terminates_running_process() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn("sleep 30", &cwd, false).await.unwrap();
        let running = registry.list().await;
        assert_eq!(running[0].state, ProcessState::Running);
        registry.kill(started.id).await.unwrap();
        wait_until_exited(&registry, started.id).await;
    }

    #[tokio::test]
    async fn stdin_write_reaches_process() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn("cat", &cwd, false).await.unwrap();
        registry.write_stdin(started.id, "typed\n").await.unwrap();
        for _ in 0..200 {
            let chunk = registry.read_new(started.id).await.unwrap();
            if chunk.text.contains("typed") {
                registry.kill(started.id).await.unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("stdin was not echoed back");
    }

    #[tokio::test]
    async fn write_to_exited_process_errors() {
        let (registry, _events, _wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn("true", &cwd, false).await.unwrap();
        wait_until_exited(&registry, started.id).await;
        let result = registry.write_stdin(started.id, "x\n").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn watch_can_be_toggled() {
        let (registry, _events, mut wake) = harness();
        let cwd = std::env::temp_dir();
        let started = registry.spawn("sleep 30", &cwd, false).await.unwrap();
        registry.set_watch(started.id, true).await.unwrap();
        registry.write_stdin(started.id, "").await.ok();
        registry.set_watch(started.id, false).await.unwrap();
        registry.kill(started.id).await.unwrap();
        let _ = wake.try_recv();
    }
}
