use clap::{Parser, Subcommand, ValueEnum};
use goat_auth::CredentialService;

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
    #[command(subcommand)]
    Search(SearchCommand),
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
        #[arg(long, value_enum, default_value = "model")]
        service: AuthServiceArg,
    },
    #[command(visible_alias = "ls")]
    List,
    #[command(visible_alias = "rm")]
    Logout {
        provider: String,
        #[arg(long, short)]
        account: Option<String>,
        #[arg(long, value_enum, default_value = "model")]
        service: AuthServiceArg,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AuthServiceArg {
    Model,
    Search,
}

impl From<AuthServiceArg> for CredentialService {
    fn from(value: AuthServiceArg) -> Self {
        match value {
            AuthServiceArg::Model => Self::Model,
            AuthServiceArg::Search => Self::Search,
        }
    }
}

#[derive(Subcommand)]
pub enum SearchCommand {
    Add {
        provider: String,
        #[arg(long, short)]
        account: String,
        #[arg(long)]
        endpoint: Option<String>,
        #[arg(long)]
        engine: Option<String>,
        #[arg(long)]
        default: bool,
    },
    Default {
        target: String,
    },
    List,
    Remove {
        target: String,
    },
}
