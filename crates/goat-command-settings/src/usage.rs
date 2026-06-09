use goat_command::{Command, CommandEffect};

pub struct Usage;

impl Command for Usage {
    fn name(&self) -> &'static str {
        "usage"
    }

    fn description(&self) -> &'static str {
        "show token usage and rate limits"
    }

    fn run(&self, _args: &str) -> CommandEffect {
        CommandEffect::OpenUsage
    }
}
