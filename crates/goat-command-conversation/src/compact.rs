use goat_command::{Command, CommandEffect};

pub struct Compact;

impl Command for Compact {
    fn name(&self) -> &'static str {
        "compact"
    }

    fn description(&self) -> &'static str {
        "summarize the conversation to free context"
    }

    fn run(&self, args: &str) -> CommandEffect {
        let trimmed = args.trim();
        let instructions = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        CommandEffect::CompactConversation(instructions)
    }
}
