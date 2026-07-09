mod auth;
mod cli;
mod cli_ui;
mod headless;
mod logging;
mod provider_table;
mod search;
mod style;
mod theme;
mod update;

use clap::Parser;
use color_eyre::eyre::eyre;

use crate::{
    cli::{Cli, Command, DaemonCommand, RemoteCommand, WorktreeCommand},
    style::{ColorMode, Palette, print_row},
};

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let cli = Cli::parse();

    if let Err(message) = goat_config::check_legacy_layout() {
        return Err(eyre!(message));
    }

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
        Some(Command::Provider(command)) => {
            reject_worktree(cli.worktree.as_ref())?;
            reject_continue(cli.r#continue)?;
            auth::run_provider(command).await
        }
        Some(Command::Search(command)) => {
            reject_worktree(cli.worktree.as_ref())?;
            reject_continue(cli.r#continue)?;
            search::run(command)
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
        None => {
            if cli.headless || cli.print {
                run_headless(cli.worktree, cli.r#continue, &cli.protocol, cli.print).await
            } else {
                run_tui(cli.worktree, cli.r#continue).await
            }
        }
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

async fn connect_session(
    worktree_label: Option<String>,
    r#continue: bool,
) -> color_eyre::Result<goat_client::Attachment> {
    let cwd = if let Some(label) = worktree_label.as_deref() {
        goat_worktree::enter(label)?
    } else {
        std::env::current_dir()?
    };

    let socket_path = goat_config::socket_path()
        .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
    let daemon_exe = std::env::current_exe()?;
    let resume = if r#continue {
        goat_wire::ResumeMode::Latest {}
    } else {
        goat_wire::ResumeMode::New {}
    };

    goat_client::connect(&socket_path, &daemon_exe, cwd, resume)
        .await
        .map_err(color_eyre::Report::from)
}

async fn run_tui(worktree_label: Option<String>, r#continue: bool) -> color_eyre::Result<()> {
    goat_tui::install_hooks()?;
    let _guard = logging::init();

    let config = goat_config::Config::load();
    let theme = match config.theme {
        goat_config::ThemeChoice::Dark => goat_tui::Theme::dark(),
        goat_config::ThemeChoice::Light => goat_tui::Theme::light(),
    };

    let attachment = connect_session(worktree_label, r#continue).await?;
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

async fn run_headless(
    worktree_label: Option<String>,
    r#continue: bool,
    protocol: &str,
    one_shot: bool,
) -> color_eyre::Result<()> {
    let _guard = logging::init();

    let codec = headless::codec_for(protocol)?;
    let attachment = connect_session(worktree_label, r#continue).await?;
    let goat_client::Attachment {
        ops, events, pump, ..
    } = attachment;

    let exit = headless::run(ops, events, codec, one_shot).await;
    pump.abort();
    match exit {
        headless::Exit::Ok => std::process::exit(0),
        headless::Exit::Disconnected => {
            eprintln!("headless: daemon connection closed");
            std::process::exit(1);
        }
    }
}

fn install_daemon_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info.location().map_or_else(
            || "unknown".to_owned(),
            |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
        );
        let message = info.payload().downcast_ref::<&str>().map_or_else(
            || {
                info.payload()
                    .downcast_ref::<String>()
                    .map_or("<non-string panic payload>", String::as_str)
                    .to_owned()
            },
            |s| (*s).to_owned(),
        );
        tracing::error!(location, message, "daemon panicked");
        previous(info);
    }));
}

async fn run_daemon_command(command: DaemonCommand) -> color_eyre::Result<()> {
    let socket_path = goat_config::socket_path()
        .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
    match command {
        DaemonCommand::Serve => {
            let _guard = logging::init();
            install_daemon_panic_hook();
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
        DaemonCommand::List => {
            let sessions = goat_client::status(&socket_path).await?;
            let color = ColorMode::detect();
            if sessions.is_empty() {
                println!("{}", color.paint("no live sessions", Palette::Muted));
            } else {
                println!(
                    "  {} {} {} {} {}",
                    color.cell("session", Palette::Muted, 10),
                    color.cell("state", Palette::Muted, 14),
                    color.cell("windows", Palette::Muted, 8),
                    color.cell("age", Palette::Muted, 8),
                    color.paint("cwd", Palette::Muted)
                );
                for session in sessions {
                    let (state, palette) = daemon_state(session.state);
                    println!(
                        "{} {} {} {} {} {}",
                        color.paint("●", palette),
                        color.cell(format!("#{}", session.session.0), Palette::Provider, 10),
                        color.cell(state, palette, 14),
                        color.cell(session.windows.to_string(), Palette::Value, 8),
                        color.cell(format!("{}s", session.age_ms / 1000), Palette::Value, 8),
                        color.paint(session.cwd, Palette::Value)
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

fn daemon_state(state: goat_wire::SessionLiveState) -> (&'static str, Palette) {
    match state {
        goat_wire::SessionLiveState::Idle {} => ("idle", Palette::Local),
        goat_wire::SessionLiveState::Active {} => ("active", Palette::Success),
        goat_wire::SessionLiveState::WaitingOnAsk {} => ("waiting", Palette::Warning),
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
            let color = ColorMode::detect();
            println!("{}", color.paint("pairing", Palette::Provider));
            print_row(color, "code", &info.code, Palette::Value);
            print_row(
                color,
                "fingerprint",
                &info.server_fingerprint,
                Palette::Value,
            );
            print_row(
                color,
                "address",
                if info.advertised.is_empty() {
                    "none configured".to_owned()
                } else {
                    info.advertised.join(", ")
                },
                Palette::Value,
            );
            print_pairing_qr(&info);
            Ok(())
        }
        RemoteCommand::List => {
            let devices = goat_client::list_devices(&socket_path).await?;
            let color = ColorMode::detect();
            if devices.is_empty() {
                println!("{}", color.paint("no paired devices", Palette::Muted));
            } else {
                println!(
                    "  {} {} {}",
                    color.cell("device", Palette::Muted, 20),
                    color.cell("label", Palette::Muted, 18),
                    color.paint("paired", Palette::Muted)
                );
                for device in devices {
                    println!(
                        "{} {} {} {}",
                        color.paint("●", Palette::Success),
                        color.cell(device.id, Palette::Provider, 20),
                        color.cell(device.label, Palette::Value, 18),
                        color.paint(device.paired_at.to_string(), Palette::Value)
                    );
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
                .render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .module_dimensions(1, 1)
                .build();
            println!("{rendered}");
        }
        Err(_) => {
            println!("(could not render QR; use the values above)");
        }
    }
}
