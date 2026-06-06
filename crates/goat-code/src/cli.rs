use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "goat", version, about = "goat — a terminal coding agent")]
pub struct Cli {
    #[arg(long)]
    pub print_log_path: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    Update,
    #[command(subcommand)]
    Auth(AuthCommand),
}

#[derive(Subcommand)]
pub enum AuthCommand {
    #[command(visible_alias = "add")]
    Login {
        provider: String,
        #[arg(long, short)]
        account: Option<String>,
        #[arg(long)]
        key: Option<String>,
    },
    #[command(visible_alias = "ls")]
    List,
    #[command(visible_alias = "rm")]
    Logout {
        provider: String,
        #[arg(long, short)]
        account: Option<String>,
    },
}
