use goat_command::{Command, CommandEffect};

pub struct Rename;

impl Command for Rename {
    fn name(&self) -> &'static str {
        "rename"
    }

    fn description(&self) -> &'static str {
        "rename the current conversation: /rename <title>"
    }

    fn run(&self, args: &str) -> CommandEffect {
        let title = args.trim();
        if title.is_empty() {
            return CommandEffect::Error("usage: /rename <title>".to_owned());
        }
        CommandEffect::RenameConversation(title.to_owned())
    }
}
