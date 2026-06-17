mod conn;
mod manager;
mod remote;
mod session;

use std::path::{Path, PathBuf};

use goat_wire::transport;

use crate::manager::Manager;

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("a daemon is already running at {0}")]
    AlreadyRunning(PathBuf),
    #[error("remote error: {0}")]
    Remote(#[from] goat_remote::RemoteError),
}

pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub auth_path: PathBuf,
    pub db_path: PathBuf,
    pub remote: Option<RemoteSettings>,
}

pub struct RemoteSettings {
    pub remote_dir: PathBuf,
    pub bind: std::net::SocketAddr,
    pub advertised: Vec<String>,
}

pub async fn serve(config: DaemonConfig) -> Result<(), DaemonError> {
    let listener = bind(&config.socket_path)?;
    sweep_orphaned_turns(&config.db_path).await;
    let manager = Manager::new(config.auth_path, config.db_path);
    let shutdown = tokio_util::sync::CancellationToken::new();
    tracing::info!(socket = %config.socket_path.display(), "daemon listening");

    if let Some(remote_settings) = config.remote {
        spawn_remote(&manager, &shutdown, remote_settings)?;
    }

    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!("daemon shutting down");
                break;
            }
            accepted = listener.accept() => match accepted {
                Ok(stream) => {
                    let manager = manager.clone();
                    let shutdown = shutdown.clone();
                    tokio::spawn(conn::handle_connection(stream, manager, shutdown));
                }
                Err(err) => {
                    tracing::warn!(%err, "accept failed");
                }
            },
        }
    }

    transport::cleanup(&config.socket_path);
    Ok(())
}

fn spawn_remote(
    manager: &Manager,
    shutdown: &tokio_util::sync::CancellationToken,
    settings: RemoteSettings,
) -> Result<(), DaemonError> {
    let devices_path = settings.remote_dir.join("devices.json");
    let devices = goat_remote::Devices::load(devices_path)?;
    let config = goat_remote::RemoteConfig {
        remote_dir: settings.remote_dir,
        bind: settings.bind,
        advertised: settings.advertised,
    };
    let server = goat_remote::RemoteServer::new(config, devices.clone())?;
    manager.set_remote(
        server.pairing(),
        server.devices(),
        server.server_fingerprint().to_owned(),
        server.advertised().to_vec(),
    );
    let handler = remote::handler(manager.clone(), devices, shutdown.clone());
    let shutdown = shutdown.clone();
    tokio::spawn(async move {
        if let Err(err) = server.run(handler, shutdown).await {
            tracing::warn!(%err, "remote server stopped");
        }
    });
    Ok(())
}

fn bind(socket_path: &Path) -> Result<transport::Listener, DaemonError> {
    if transport::exists(socket_path) && transport::probe_alive(socket_path) {
        return Err(DaemonError::AlreadyRunning(socket_path.to_path_buf()));
    }
    Ok(transport::bind(socket_path)?)
}

async fn sweep_orphaned_turns(db_path: &Path) {
    let Ok(store) = goat_store::Store::open(db_path) else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    match store.mark_running_turns_interrupted(now).await {
        Ok(n) if n > 0 => tracing::info!(count = n, "marked orphaned turns interrupted"),
        Ok(_) => {}
        Err(err) => tracing::warn!(%err, "failed to sweep orphaned turns"),
    }
}
