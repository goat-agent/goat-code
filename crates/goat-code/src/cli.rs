use clap::{Parser, Subcommand};

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

    use super::{Cli, Command, WorktreeCommand};

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
