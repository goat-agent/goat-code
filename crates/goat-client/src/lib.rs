use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use goat_protocol::{Event, Op};
use goat_wire::transport::{self, Stream};
use goat_wire::{
    ClientConn, ClientFrame, PROTOCOL_VERSION, ResumeMode, ServerFrame, SessionId, WireError,
};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use crate::idmap::IdMap;

mod idmap;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("wire error: {0}")]
    Wire(#[from] WireError),
    #[error("daemon protocol version {0} does not match client {PROTOCOL_VERSION}")]
    VersionMismatch(u32),
    #[error("unexpected daemon response during handshake")]
    Handshake,
    #[error("daemon did not open a session: {0}")]
    OpenFailed(String),
    #[error("could not start daemon: {0}")]
    SpawnFailed(String),
}

pub struct Attachment {
    pub ops: mpsc::Sender<Op>,
    pub events: mpsc::Receiver<Event>,
    pub presence: mpsc::Receiver<usize>,
    pub client_id: u64,
    pub pump: JoinHandle<()>,
}

const OPS_CAPACITY: usize = 32;
const EVENTS_CAPACITY: usize = 512;
const PRESENCE_CAPACITY: usize = 16;

pub async fn connect(
    socket_path: &Path,
    daemon_exe: &Path,
    cwd: PathBuf,
    resume: ResumeMode,
) -> Result<Attachment, ClientError> {
    let stream = connect_or_spawn(socket_path, daemon_exe).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);

    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    let client_id = match conn.recv().await? {
        ServerFrame::Welcome { version, client_id } => {
            if version != PROTOCOL_VERSION {
                return Err(ClientError::VersionMismatch(version));
            }
            client_id.0
        }
        ServerFrame::VersionMismatch { daemon_version } => {
            return Err(ClientError::VersionMismatch(daemon_version));
        }
        _ => return Err(ClientError::Handshake),
    };

    conn.send(&ClientFrame::OpenSession {
        cwd: cwd.display().to_string(),
        resume,
    })
    .await?;
    let session = match conn.recv().await? {
        ServerFrame::SessionOpened { session, .. } => session,
        ServerFrame::Error { message } => return Err(ClientError::OpenFailed(message)),
        _ => return Err(ClientError::Handshake),
    };

    Ok(spawn_pumps(conn, session, client_id))
}

fn spawn_pumps(conn: ClientConn<Stream>, session: SessionId, client_id: u64) -> Attachment {
    let (ops_tx, mut ops_rx) = mpsc::channel::<Op>(OPS_CAPACITY);
    let (events_tx, events_rx) = mpsc::channel::<Event>(EVENTS_CAPACITY);
    let (presence_tx, presence_rx) = mpsc::channel::<usize>(PRESENCE_CAPACITY);

    let (sink, mut source) = conn.split();
    let mut sink = Box::pin(sink);
    let idmap = Arc::new(Mutex::new(IdMap::new()));

    let outbound_map = idmap.clone();
    let (resync_tx, mut resync_rx) = mpsc::channel::<()>(4);

    let outbound = tokio::spawn(async move {
        use futures::SinkExt;
        loop {
            tokio::select! {
                biased;
                Some(()) = resync_rx.recv() => {
                    if sink.send(ClientFrame::Attach { session }).await.is_err() {
                        break;
                    }
                }
                maybe_op = ops_rx.recv() => {
                    let Some(op) = maybe_op else { break };
                    let frame = match op {
                        Op::Shutdown {} => ClientFrame::Goodbye {},
                        Op::Interrupt { .. }
                        | Op::Answer { .. }
                        | Op::DequeueMessage { .. }
                        | Op::ResolvePlan { .. } => {
                            let mut op = op;
                            outbound_map.lock().await.translate_outbound(&mut op);
                            ClientFrame::Control { session, op }
                        }
                        other => {
                            let correlation = submit_correlation(&other);
                            ClientFrame::Submit {
                                session,
                                correlation,
                                op: other,
                            }
                        }
                    };
                    if sink.send(frame).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let pump = tokio::spawn(async move {
        use futures::StreamExt;
        let mut expected_seq: Option<u64> = None;
        while let Some(item) = source.next().await {
            let Ok(frame) = item else { break };
            if let ServerFrame::CorrelationAssigned {
                correlation, task, ..
            } = &frame
            {
                idmap.lock().await.record_correlation(*correlation, *task);
                continue;
            }
            if let ServerFrame::Presence { clients, .. } = &frame {
                let _ = presence_tx.try_send(clients.len());
                continue;
            }
            if let ServerFrame::Snapshot { watermark, .. } = &frame {
                expected_seq = Some(*watermark);
            }
            if let ServerFrame::Event { seq, .. } = &frame {
                match expected_seq {
                    Some(exp) if *seq > exp => {
                        let _ = resync_tx.try_send(());
                        expected_seq = Some(*seq + 1);
                    }
                    _ => expected_seq = Some(*seq + 1),
                }
            }
            if let Some(mut event) = frame_to_event(frame) {
                idmap.lock().await.translate_inbound(&mut event);
                if events_tx.send(event).await.is_err() {
                    break;
                }
            }
        }
        outbound.abort();
    });

    Attachment {
        ops: ops_tx,
        events: events_rx,
        presence: presence_rx,
        client_id,
        pump,
    }
}

fn submit_correlation(op: &Op) -> u64 {
    match op {
        Op::SubmitMessage { id, .. } | Op::SubmitShell { id, .. } | Op::Compact { id, .. } => id.0,
        _ => 0,
    }
}

fn frame_to_event(frame: ServerFrame) -> Option<Event> {
    match frame {
        ServerFrame::Event { event, .. } => Some(event),
        ServerFrame::Snapshot {
            target,
            transcript,
            context_tokens,
            compaction_threshold,
            mode,
            ..
        } => target.map(|target| Event::ConversationRestored {
            target,
            entries: transcript,
            context_tokens,
            compaction_threshold,
            mode,
        }),
        ServerFrame::Error { message } => Some(Event::Error { id: None, message }),
        _ => None,
    }
}

pub async fn status(socket_path: &Path) -> Result<Vec<goat_wire::SessionInfo>, ClientError> {
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    expect_welcome(&mut conn).await?;
    conn.send(&ClientFrame::ListSessions {}).await?;
    match conn.recv().await? {
        ServerFrame::Sessions { sessions } => Ok(sessions),
        _ => Err(ClientError::Handshake),
    }
}

pub async fn stop(socket_path: &Path) -> Result<(), ClientError> {
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    expect_welcome(&mut conn).await?;
    conn.send(&ClientFrame::StopDaemon {}).await?;
    Ok(())
}

pub async fn kill_session(socket_path: &Path, session: u64) -> Result<(), ClientError> {
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    expect_welcome(&mut conn).await?;
    conn.send(&ClientFrame::KillSession {
        session: SessionId(session),
    })
    .await?;
    Ok(())
}

pub struct PairingInfo {
    pub code: String,
    pub server_fingerprint: String,
    pub advertised: Vec<String>,
}

pub async fn pair_device(socket_path: &Path, label: String) -> Result<PairingInfo, ClientError> {
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    expect_welcome(&mut conn).await?;
    conn.send(&ClientFrame::PairDevice { label }).await?;
    match conn.recv().await? {
        ServerFrame::PairingCode {
            code,
            server_fingerprint,
            advertised,
        } => Ok(PairingInfo {
            code,
            server_fingerprint,
            advertised,
        }),
        ServerFrame::Error { message } => Err(ClientError::OpenFailed(message)),
        _ => Err(ClientError::Handshake),
    }
}

pub async fn list_devices(socket_path: &Path) -> Result<Vec<goat_wire::DeviceInfo>, ClientError> {
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    expect_welcome(&mut conn).await?;
    conn.send(&ClientFrame::ListDevices {}).await?;
    match conn.recv().await? {
        ServerFrame::Devices { devices } => Ok(devices),
        ServerFrame::Error { message } => Err(ClientError::OpenFailed(message)),
        _ => Err(ClientError::Handshake),
    }
}

pub async fn revoke_device(socket_path: &Path, device: String) -> Result<bool, ClientError> {
    let stream = transport::connect(socket_path).await?;
    let mut conn: ClientConn<Stream> = ClientConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await?;
    expect_welcome(&mut conn).await?;
    conn.send(&ClientFrame::RevokeDevice { device }).await?;
    match conn.recv().await? {
        ServerFrame::DeviceRevoked { ok } => Ok(ok),
        ServerFrame::Error { message } => Err(ClientError::OpenFailed(message)),
        _ => Err(ClientError::Handshake),
    }
}

async fn expect_welcome(conn: &mut ClientConn<Stream>) -> Result<(), ClientError> {
    match conn.recv().await? {
        ServerFrame::Welcome { version, .. } => {
            if version != PROTOCOL_VERSION {
                return Err(ClientError::VersionMismatch(version));
            }
            Ok(())
        }
        ServerFrame::VersionMismatch { daemon_version } => {
            Err(ClientError::VersionMismatch(daemon_version))
        }
        _ => Err(ClientError::Handshake),
    }
}

async fn connect_or_spawn(socket_path: &Path, daemon_exe: &Path) -> Result<Stream, ClientError> {
    if let Ok(stream) = transport::connect(socket_path).await {
        return Ok(stream);
    }
    spawn_daemon(daemon_exe)?;
    for _ in 0..50 {
        if let Ok(stream) = transport::connect(socket_path).await {
            return Ok(stream);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(ClientError::SpawnFailed(
        "daemon did not become reachable".to_owned(),
    ))
}

fn spawn_daemon(daemon_exe: &Path) -> Result<(), ClientError> {
    use std::process::{Command, Stdio};
    Command::new(daemon_exe)
        .arg("daemon")
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| ClientError::SpawnFailed(e.to_string()))?;
    Ok(())
}
