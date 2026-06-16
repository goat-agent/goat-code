mod auth;
mod cli;
mod logging;
mod update;

use clap::Parser;
use color_eyre::eyre::eyre;

use crate::cli::{Cli, Command, WorktreeCommand};

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let cli = Cli::parse();

    if cli.print_log_path {
        reject_worktree(cli.worktree.as_ref())?;
        if let Some(dir) = goat_config::log_dir() {
            println!("{}", dir.display());
        }
        return Ok(());
    }

    match cli.command {
        Some(Command::Update { force }) => {
            reject_worktree(cli.worktree.as_ref())?;
            update::run(force).await
        }
        Some(Command::Auth(command)) => {
            reject_worktree(cli.worktree.as_ref())?;
            auth::run(command).await
        }
        Some(Command::Worktree(command)) => {
            reject_worktree(cli.worktree.as_ref())?;
            let result = match command {
                WorktreeCommand::List => goat_worktree::list(),
                WorktreeCommand::Remove { label } => goat_worktree::remove(&label),
            };
            result.map_err(color_eyre::Report::from)
        }
        None => run_tui(cli.worktree).await,
    }
}

fn reject_worktree(worktree: Option<&String>) -> color_eyre::Result<()> {
    if worktree.is_some() {
        return Err(eyre!("--worktree can only be used when launching the TUI"));
    }
    Ok(())
}

async fn run_tui(worktree_label: Option<String>) -> color_eyre::Result<()> {
    if let Some(label) = worktree_label.as_deref() {
        goat_worktree::enter(label)?;
    }

    goat_tui::install_hooks()?;
    let _guard = logging::init();

    let config = goat_config::Config::load();
    let theme = match config.theme {
        goat_config::ThemeChoice::Dark => goat_tui::Theme::dark(),
        goat_config::ThemeChoice::Light => goat_tui::Theme::light(),
    };

    let auth_path = goat_config::auth_path()
        .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
    let db_path = goat_config::db_path()
        .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
    let credentials = goat_auth::CredentialStore::new(auth_path);
    let store = goat_store::Store::open(&db_path)?;
    let registry = goat_providers::Registry::new(&credentials);
    let agent = goat_agent::GoatAgent::new(registry, store, credentials, None).await;

    let session = goat_core::Session::spawn(agent);
    let (ops, events, handle) = session.into_parts();
    goat_tui::run(ops, events, theme).await?;
    handle.await.ok();
    Ok(())
}
