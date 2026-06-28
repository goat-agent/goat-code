use clap::{Parser, Subcommand, ValueEnum};
use goat_auth::CredentialService;

#[derive(Parser)]
#[command(name = "goat", version, about = "goat — a terminal coding agent")]
pub struct Cli {
    #[arg(long)]
    pub print_log_path: bool,

    #[arg(long, short = 'w', value_name = "NAME")]
    pub worktree: Option<String>,

    #[arg(long, short = 'c')]
    pub r#continue: bool,

    #[arg(long)]
    pub headless: bool,

    #[arg(long, short = 'p')]
    pub print: bool,

    #[arg(long, value_name = "NAME", default_value = "json")]
    pub protocol: String,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    Update {
        #[arg(long)]
        force: bool,
    },
    #[command(subcommand)]
    Auth(AuthCommand),
    #[command(
        subcommand,
        about = "Discover, connect, and disconnect model providers",
        after_help = "Common flow:\n  goat provider list\n  goat provider info kimi-code\n  goat provider login kimi-code\n  goat provider accounts\n\nUse `goat provider accounts` for stored credentials only. Use `goat provider list` to discover available providers."
    )]
    Provider(ProviderCommand),
    #[command(subcommand)]
    Search(SearchCommand),
    #[command(subcommand)]
    Worktree(WorktreeCommand),
    #[command(subcommand)]
    Daemon(DaemonCommand),
    #[command(subcommand)]
    Remote(RemoteCommand),
}

#[derive(Subcommand)]
pub enum RemoteCommand {
    Pair {
        #[arg(long, short)]
        label: Option<String>,
    },
    #[command(visible_alias = "ls")]
    Devices,
    #[command(visible_alias = "rm")]
    Revoke { device: String },
}

#[derive(Subcommand)]
pub enum DaemonCommand {
    Serve,
    #[command(visible_alias = "ls")]
    Status,
    Stop,
    Kill {
        session: u64,
    },
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

#[derive(Subcommand)]
pub enum ProviderCommand {
    #[command(
        visible_alias = "add",
        about = "Connect a provider account",
        after_help = "Examples:\n  goat provider login openrouter --key sk-...\n  goat provider login kimi-code\n  goat provider login zai-coding --key sk-...\n  goat provider login qwen --endpoint https://dashscope-us.aliyuncs.com/compatible-mode/v1 --key sk-...\n\nRun `goat provider list` to see available providers. Run `goat provider info <provider>` for setup details."
    )]
    Login {
        #[arg(help = "Provider id, for example openrouter, kimi-code, or zai-coding")]
        provider: String,
        #[arg(long, short, help = "Account name to store, default: default")]
        account: Option<String>,
        #[arg(long, help = "API key for API-key providers")]
        key: Option<String>,
        #[arg(
            long,
            value_name = "URL",
            help = "Qwen DashScope OpenAI-compatible endpoint"
        )]
        endpoint: Option<String>,
    },
    #[command(visible_alias = "ls", about = "List available model providers")]
    List {
        #[arg(
            long,
            help = "Show stored provider accounts instead of provider discovery"
        )]
        accounts: bool,
    },
    #[command(about = "Show stored model provider accounts")]
    Accounts,
    #[command(about = "Show setup details for one provider")]
    Info { provider: String },
    #[command(visible_alias = "rm", about = "Disconnect a stored provider account")]
    Logout {
        provider: String,
        #[arg(long, short)]
        account: Option<String>,
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

#[derive(Subcommand)]
pub enum WorktreeCommand {
    #[command(visible_alias = "ls")]
    List,
    #[command(visible_alias = "rm")]
    Remove { label: String },
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command, ProviderCommand, WorktreeCommand};

    #[test]
    fn parses_short_worktree_flag() {
        let cli = Cli::try_parse_from(["goat", "-w", "plan"]).unwrap();
        assert_eq!(cli.worktree.as_deref(), Some("plan"));
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_long_worktree_flag() {
        let cli = Cli::try_parse_from(["goat", "--worktree", "plan"]).unwrap();
        assert_eq!(cli.worktree.as_deref(), Some("plan"));
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_short_continue_flag() {
        let cli = Cli::try_parse_from(["goat", "-c"]).unwrap();
        assert!(cli.r#continue);
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_long_continue_flag() {
        let cli = Cli::try_parse_from(["goat", "--continue"]).unwrap();
        assert!(cli.r#continue);
        assert!(cli.command.is_none());
    }

    #[test]
    fn continue_defaults_off() {
        let cli = Cli::try_parse_from(["goat"]).unwrap();
        assert!(!cli.r#continue);
    }

    #[test]
    fn parses_worktree_list() {
        let cli = Cli::try_parse_from(["goat", "worktree", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Worktree(WorktreeCommand::List))
        ));
    }

    #[test]
    fn parses_worktree_remove() {
        let cli = Cli::try_parse_from(["goat", "worktree", "remove", "plan"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Worktree(WorktreeCommand::Remove { label })) if label == "plan"
        ));
    }

    #[test]
    fn parses_update() {
        let cli = Cli::try_parse_from(["goat", "update"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Update { force: false })
        ));
    }

    #[test]
    fn parses_update_force() {
        let cli = Cli::try_parse_from(["goat", "update", "--force"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Update { force: true })));
    }

    #[test]
    fn headless_defaults_off() {
        let cli = Cli::try_parse_from(["goat"]).unwrap();
        assert!(!cli.headless);
        assert!(!cli.print);
        assert_eq!(cli.protocol, "json");
    }

    #[test]
    fn parses_provider_login() {
        let cli = Cli::try_parse_from(["goat", "provider", "login", "openrouter", "--key", "sk"])
            .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::Login { provider, key, .. }))
                if provider == "openrouter" && key.as_deref() == Some("sk")
        ));
    }

    #[test]
    fn parses_provider_list() {
        let cli = Cli::try_parse_from(["goat", "provider", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::List { accounts: false }))
        ));
    }

    #[test]
    fn parses_provider_list_accounts() {
        let cli = Cli::try_parse_from(["goat", "provider", "list", "--accounts"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::List { accounts: true }))
        ));
    }

    #[test]
    fn parses_provider_accounts() {
        let cli = Cli::try_parse_from(["goat", "provider", "accounts"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::Accounts))
        ));
    }

    #[test]
    fn parses_provider_login_endpoint() {
        let cli = Cli::try_parse_from([
            "goat",
            "provider",
            "login",
            "qwen",
            "--endpoint",
            "https://dashscope-us.aliyuncs.com/compatible-mode/v1",
            "--key",
            "sk",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::Login { provider, endpoint, .. }))
                if provider == "qwen" && endpoint.as_deref() == Some("https://dashscope-us.aliyuncs.com/compatible-mode/v1")
        ));
    }

    #[test]
    fn provider_login_help_mentions_list() {
        let Err(error) = Cli::try_parse_from(["goat", "provider", "login", "--help"]) else {
            panic!("expected help error");
        };
        assert!(error.to_string().contains("goat provider list"));
    }

    #[test]
    fn parses_provider_info() {
        let cli = Cli::try_parse_from(["goat", "provider", "info", "kimi-code"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::Info { provider })) if provider == "kimi-code"
        ));
    }

    #[test]
    fn provider_help_mentions_accounts() {
        let Err(error) = Cli::try_parse_from(["goat", "provider", "--help"]) else {
            panic!("expected help error");
        };
        let help = error.to_string();
        assert!(help.contains("Discover, connect, and disconnect model providers"));
        assert!(help.contains("goat provider accounts"));
    }

    #[test]
    fn parses_provider_logout() {
        let cli = Cli::try_parse_from(["goat", "provider", "logout", "openrouter"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Provider(ProviderCommand::Logout { provider, .. })) if provider == "openrouter"
        ));
    }

    #[test]
    fn provider_does_not_accept_service() {
        let result = Cli::try_parse_from([
            "goat",
            "provider",
            "login",
            "openrouter",
            "--service",
            "search",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn auth_still_accepts_service() {
        let cli = Cli::try_parse_from([
            "goat",
            "auth",
            "login",
            "brave",
            "--service",
            "search",
            "--key",
            "key",
        ])
        .unwrap();
        assert!(matches!(cli.command, Some(Command::Auth(_))));
    }

    #[test]
    fn parses_headless_flag() {
        let cli = Cli::try_parse_from(["goat", "--headless"]).unwrap();
        assert!(cli.headless);
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_print_flag() {
        let cli = Cli::try_parse_from(["goat", "-p"]).unwrap();
        assert!(cli.print);
    }

    #[test]
    fn parses_protocol_flag() {
        let cli = Cli::try_parse_from(["goat", "--headless", "--protocol", "json"]).unwrap();
        assert_eq!(cli.protocol, "json");
    }
}
