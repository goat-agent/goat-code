use std::path::PathBuf;

use futures::{SinkExt, StreamExt};
use goat_wire::transport::Stream;
use goat_wire::{ClientFrame, PROTOCOL_VERSION, ServerConn, ServerFrame};
use tokio::sync::mpsc;

use crate::manager::Manager;

const CLIENT_QUEUE: usize = 1024;

pub(crate) async fn handle_connection(
    stream: Stream,
    manager: Manager,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let mut conn: ServerConn<Stream> = ServerConn::new(stream);

    if let Ok(ClientFrame::Hello { version }) = conn.recv().await {
        if version != PROTOCOL_VERSION {
            let _ = conn
                .send(&ServerFrame::VersionMismatch {
                    daemon_version: PROTOCOL_VERSION,
                })
                .await;
            return;
        }
    } else {
        let _ = conn
            .send(&ServerFrame::Error {
                message: "expected Hello".to_owned(),
            })
            .await;
        return;
    }

    let client_id = manager.next_client_id();
    if conn
        .send(&ServerFrame::Welcome {
            version: PROTOCOL_VERSION,
            client_id,
        })
        .await
        .is_err()
    {
        return;
    }

    let (out_tx, mut out_rx) = mpsc::channel::<ServerFrame>(CLIENT_QUEUE);

    let (sink, mut source) = conn.split();
    let mut sink = Box::pin(sink);

    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if sink.send(frame).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(frame)) = source.next().await {
        if !dispatch(&manager, client_id, &out_tx, &shutdown, frame).await {
            break;
        }
    }

    manager.drop_client(client_id).await;
    writer.abort();
}

async fn dispatch(
    manager: &Manager,
    client_id: goat_wire::ClientId,
    out_tx: &mpsc::Sender<ServerFrame>,
    shutdown: &tokio_util::sync::CancellationToken,
    frame: ClientFrame,
) -> bool {
    match frame {
        ClientFrame::Hello { .. } => true,
        ClientFrame::OpenSession { cwd, resume } => {
            let cwd_path = PathBuf::from(&cwd);
            let existing = if matches!(resume, goat_wire::ResumeMode::Latest) {
                manager.find_live_by_cwd(&cwd_path).await
            } else {
                None
            };
            let opened = match existing {
                Some(session) => Ok(session),
                None => manager.open_session(cwd_path, resume).await,
            };
            match opened {
                Ok(session) => {
                    let _ = out_tx
                        .send(ServerFrame::SessionOpened {
                            session,
                            correlation_ok: true,
                        })
                        .await;
                    let _ = manager.subscribe(session, client_id, out_tx.clone()).await;
                }
                Err(message) => {
                    let _ = out_tx.send(ServerFrame::Error { message }).await;
                }
            }
            true
        }
        ClientFrame::Attach { session } => {
            if let Err(message) = manager.subscribe(session, client_id, out_tx.clone()).await {
                let _ = out_tx.send(ServerFrame::Error { message }).await;
            }
            true
        }
        ClientFrame::Detach { session } => {
            manager.unsubscribe(session, client_id).await;
            let _ = out_tx.send(ServerFrame::Detached { session }).await;
            true
        }
        ClientFrame::Submit {
            session,
            correlation,
            op,
        } => {
            if let Err(message) = manager.submit(session, out_tx, correlation, op).await {
                let _ = out_tx.send(ServerFrame::Error { message }).await;
            }
            true
        }
        ClientFrame::Control { session, op } => {
            if let Err(message) = manager.control(session, op).await {
                let _ = out_tx.send(ServerFrame::Error { message }).await;
            }
            true
        }
        ClientFrame::ListSessions => {
            let sessions = manager.list_sessions().await;
            let _ = out_tx.send(ServerFrame::SessionList { sessions }).await;
            true
        }
        ClientFrame::KillSession { session } => {
            if let Err(message) = manager.kill_session(session).await {
                let _ = out_tx.send(ServerFrame::Error { message }).await;
            }
            true
        }
        ClientFrame::StopDaemon => {
            shutdown.cancel();
            false
        }
        ClientFrame::Goodbye => false,
    }
}
