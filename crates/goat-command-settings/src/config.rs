use goat_command::{Command, CommandEffect};

pub struct Config;

impl Command for Config {
    fn name(&self) -> &'static str {
        "config"
    }

    fn description(&self) -> &'static str {
        "configure providers and settings"
    }

    fn run(&self, _args: &str) -> CommandEffect {
        CommandEffect::OpenConfig
    }
}
