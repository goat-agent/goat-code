mod auth;
mod cli;
mod logging;
mod update;

use clap::Parser;
use color_eyre::eyre::eyre;

use crate::cli::{Cli, Command, DaemonCommand, RemoteCommand, WorktreeCommand};

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let cli = Cli::parse();

    if cli.print_log_path {
        reject_worktree(cli.worktree.as_ref())?;
        reject_continue(cli.r#continue)?;
        if let Some(dir) = goat_config::log_dir() {
            println!("{}", dir.display());
        }
        return Ok(());
    }

    match cli.command {
        Some(Command::Update { force }) => {
            reject_worktree(cli.worktree.as_ref())?;
            reject_continue(cli.r#continue)?;
            update::run(force).await
        }
        Some(Command::Auth(command)) => {
            reject_worktree(cli.worktree.as_ref())?;
            reject_continue(cli.r#continue)?;
            auth::run(command).await
        }
        Some(Command::Worktree(command)) => {
            reject_worktree(cli.worktree.as_ref())?;
            reject_continue(cli.r#continue)?;
            let result = match command {
                WorktreeCommand::List => goat_worktree::list(),
                WorktreeCommand::Remove { label } => goat_worktree::remove(&label),
            };
            result.map_err(color_eyre::Report::from)
        }
        Some(Command::Daemon(command)) => {
            reject_worktree(cli.worktree.as_ref())?;
            reject_continue(cli.r#continue)?;
            run_daemon_command(command).await
        }
        Some(Command::Remote(command)) => {
            reject_worktree(cli.worktree.as_ref())?;
            reject_continue(cli.r#continue)?;
            run_remote_command(command).await
        }
        None => run_tui(cli.worktree, cli.r#continue).await,
    }
}

fn reject_worktree(worktree: Option<&String>) -> color_eyre::Result<()> {
    if worktree.is_some() {
        return Err(eyre!("--worktree can only be used when launching the TUI"));
    }
    Ok(())
}

fn reject_continue(r#continue: bool) -> color_eyre::Result<()> {
    if r#continue {
        return Err(eyre!("--continue can only be used when launching the TUI"));
    }
    Ok(())
}

async fn run_tui(worktree_label: Option<String>, r#continue: bool) -> color_eyre::Result<()> {
    let cwd = if let Some(label) = worktree_label.as_deref() {
        goat_worktree::enter(label)?
    } else {
        std::env::current_dir()?
    };

    goat_tui::install_hooks()?;
    let _guard = logging::init();

    let config = goat_config::Config::load();
    let theme = match config.theme {
        goat_config::ThemeChoice::Dark => goat_tui::Theme::dark(),
        goat_config::ThemeChoice::Light => goat_tui::Theme::light(),
    };

    let socket_path = goat_config::socket_path()
        .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
    let daemon_exe = std::env::current_exe()?;
    let resume = if r#continue {
        goat_wire::ResumeMode::Latest
    } else {
        goat_wire::ResumeMode::New
    };

    let attachment = goat_client::connect(&socket_path, &daemon_exe, cwd, resume).await?;
    let goat_client::Attachment {
        ops,
        events,
        presence,
        pump,
        ..
    } = attachment;

    goat_tui::run(ops, events, presence, theme, Vec::new()).await?;
    pump.abort();
    Ok(())
}

async fn run_daemon_command(command: DaemonCommand) -> color_eyre::Result<()> {
    let socket_path = goat_config::socket_path()
        .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
    match command {
        DaemonCommand::Serve => {
            let _guard = logging::init();
            let auth_path = goat_config::auth_path()
                .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
            let db_path = goat_config::db_path()
                .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
            let remote = remote_settings()?;
            goat_daemon::serve(goat_daemon::DaemonConfig {
                socket_path,
                auth_path,
                db_path,
                remote,
            })
            .await
            .map_err(color_eyre::Report::from)
        }
        DaemonCommand::Status => {
            let sessions = goat_client::status(&socket_path).await?;
            if sessions.is_empty() {
                println!("no live sessions");
            } else {
                for s in sessions {
                    let flag = match s.state {
                        goat_wire::SessionLiveState::WaitingOnAsk => " (waiting on ask)",
                        _ => "",
                    };
                    println!(
                        "#{} [{:?}] windows={} tokens={} age={}s {}{}",
                        s.session.0,
                        s.state,
                        s.windows,
                        s.tokens,
                        s.age_ms / 1000,
                        s.cwd,
                        flag
                    );
                }
            }
            Ok(())
        }
        DaemonCommand::Stop => {
            goat_client::stop(&socket_path).await?;
            println!("daemon stopped");
            Ok(())
        }
        DaemonCommand::Kill { session } => {
            goat_client::kill_session(&socket_path, session).await?;
            println!("killed session #{session}");
            Ok(())
        }
    }
}

fn remote_settings() -> color_eyre::Result<Option<goat_daemon::RemoteSettings>> {
    let config = goat_config::Config::load();
    let Some(remote_dir) = goat_config::remote_dir() else {
        return Ok(None);
    };
    let bind = config
        .remote
        .bind
        .parse()
        .map_err(|e| color_eyre::eyre::eyre!("invalid remote bind address: {e}"))?;
    Ok(Some(goat_daemon::RemoteSettings {
        remote_dir,
        bind,
        advertised: config.remote.advertised,
    }))
}

async fn run_remote_command(command: RemoteCommand) -> color_eyre::Result<()> {
    let socket_path = goat_config::socket_path()
        .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
    match command {
        RemoteCommand::Pair { label } => {
            let label = label.unwrap_or_else(|| "device".to_owned());
            let info = goat_client::pair_device(&socket_path, label).await?;
            println!("pairing code: {}", info.code);
            println!("server fingerprint: {}", info.server_fingerprint);
            if info.advertised.is_empty() {
                println!("advertised address: (none configured)");
            } else {
                println!("advertised address: {}", info.advertised.join(", "));
            }
            print_pairing_qr(&info);
            Ok(())
        }
        RemoteCommand::Devices => {
            let devices = goat_client::list_devices(&socket_path).await?;
            if devices.is_empty() {
                println!("no paired devices");
            } else {
                for d in devices {
                    println!("{} [{}] paired_at={}", d.id, d.label, d.paired_at);
                }
            }
            Ok(())
        }
        RemoteCommand::Revoke { device } => {
            let ok = goat_client::revoke_device(&socket_path, device.clone()).await?;
            if ok {
                println!("revoked device {device}");
            } else {
                println!("no such device: {device}");
            }
            Ok(())
        }
    }
}

fn print_pairing_qr(info: &goat_client::PairingInfo) {
    let address = info.advertised.first().cloned().unwrap_or_default();
    let payload = format!(
        "goat-pair:code={}&fp={}&addr={}",
        info.code, info.server_fingerprint, address
    );
    match qrcode::QrCode::new(payload.as_bytes()) {
        Ok(code) => {
            let rendered = code
                .render::<char>()
                .quiet_zone(true)
                .module_dimensions(2, 1)
                .build();
            println!("{rendered}");
        }
        Err(_) => {
            println!("(could not render QR; use the values above)");
        }
    }
}
