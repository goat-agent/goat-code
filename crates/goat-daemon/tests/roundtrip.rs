use std::path::PathBuf;
use std::time::Duration;

use goat_wire::transport::{self, Stream};
use goat_wire::{ClientConn, ClientFrame, PROTOCOL_VERSION, ResumeMode, ServerFrame, WireConn};

async fn start_daemon(dir: &std::path::Path) -> PathBuf {
    let socket = dir.join("d.sock");
    let auth = dir.join("auth.json");
    let db = dir.join("store.sqlite");
    let cfg = goat_daemon::DaemonConfig {
        socket_path: socket.clone(),
        auth_path: auth,
        db_path: db,
        remote: None,
    };
    tokio::spawn(async move {
        let _ = goat_daemon::serve(cfg).await;
    });
    for _ in 0..50 {
        if transport::connect(&socket).await.is_ok() {
            return socket;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("daemon did not start");
}

async fn connect(socket: &std::path::Path) -> ClientConn<Stream> {
    let stream = transport::connect(socket).await.unwrap();
    let mut conn: ClientConn<Stream> = WireConn::new(stream);
    conn.send(&ClientFrame::Hello {
        version: PROTOCOL_VERSION,
    })
    .await
    .unwrap();
    match conn.recv().await.unwrap() {
        ServerFrame::Welcome { version, .. } => assert_eq!(version, PROTOCOL_VERSION),
        other => panic!("expected Welcome, got {other:?}"),
    }
    conn
}

#[tokio::test]
async fn open_session_and_list() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let mut conn = connect(&socket).await;

    conn.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::New,
    })
    .await
    .unwrap();
    let session = match conn.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    let mut lister = connect(&socket).await;
    lister.send(&ClientFrame::ListSessions).await.unwrap();
    match lister.recv().await.unwrap() {
        ServerFrame::Sessions { sessions } => {
            assert!(sessions.iter().any(|s| s.session == session));
        }
        other => panic!("expected Sessions, got {other:?}"),
    }
}

#[tokio::test]
async fn submit_message_flows_back_as_events() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let mut conn = connect(&socket).await;

    conn.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::New,
    })
    .await
    .unwrap();
    let session = match conn.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    conn.send(&ClientFrame::Submit {
        session,
        correlation: 1,
        op: goat_protocol::Op::SubmitMessage {
            id: goat_protocol::TaskId(1),
            text: "hello".to_owned(),
        },
    })
    .await
    .unwrap();

    let mut saw_seq_event = false;
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_secs(5), conn.recv()).await {
            Ok(Ok(ServerFrame::Event {
                session: s, seq, ..
            })) => {
                assert_eq!(s, session);
                let _ = seq;
                saw_seq_event = true;
                break;
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) | Err(_) => break,
        }
    }
    assert!(
        saw_seq_event,
        "expected at least one seq-stamped event from the engine"
    );
}

#[tokio::test]
async fn reattach_by_cwd_returns_same_session() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;

    let mut a = connect(&socket).await;
    a.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::New,
    })
    .await
    .unwrap();
    let first = match a.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    let mut b = connect(&socket).await;
    b.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::Latest,
    })
    .await
    .unwrap();
    let second = match b.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    assert_eq!(
        first, second,
        "resume-latest must reattach to the live session in the same cwd"
    );
}

#[tokio::test]
async fn kill_session_removes_it_from_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_daemon(dir.path()).await;
    let mut conn = connect(&socket).await;
    conn.send(&ClientFrame::OpenSession {
        cwd: dir.path().display().to_string(),
        resume: ResumeMode::New,
    })
    .await
    .unwrap();
    let session = match conn.recv().await.unwrap() {
        ServerFrame::SessionOpened { session, .. } => session,
        other => panic!("expected SessionOpened, got {other:?}"),
    };

    let mut admin = connect(&socket).await;
    admin
        .send(&ClientFrame::KillSession { session })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    admin.send(&ClientFrame::ListSessions).await.unwrap();
    match admin.recv().await.unwrap() {
        ServerFrame::Sessions { sessions } => {
            assert!(!sessions.iter().any(|s| s.session == session));
        }
        other => panic!("expected Sessions, got {other:?}"),
    }
}
