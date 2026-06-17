mod auth;
mod cli;
mod logging;
mod update;

use clap::Parser;
use color_eyre::eyre::eyre;

use crate::cli::{Cli, Command, DaemonCommand, WorktreeCommand};

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
    let goat_client::Attachment { ops, events, pump } = attachment;

    goat_tui::run(ops, events, theme, Vec::new()).await?;
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
            goat_daemon::serve(goat_daemon::DaemonConfig {
                socket_path,
                auth_path,
                db_path,
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
