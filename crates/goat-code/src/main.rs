mod auth;
mod cli;
mod logging;
mod search;
mod update;

use clap::Parser;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let cli = Cli::parse();

    if cli.print_log_path {
        if let Some(dir) = goat_config::log_dir() {
            println!("{}", dir.display());
        }
        return Ok(());
    }

    match cli.command {
        Some(Command::Update) => update::run().await,
        Some(Command::Auth(command)) => auth::run(command).await,
        Some(Command::Search(command)) => search::run(command),
        None => run_tui().await,
    }
}

async fn run_tui() -> color_eyre::Result<()> {
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
    let agent = goat_agent::GoatAgent::new(registry, store, credentials, None);

    let session = goat_core::Session::spawn(agent);
    let (ops, events, handle) = session.into_parts();
    goat_tui::run(ops, events, theme).await?;
    handle.await.ok();
    Ok(())
}
