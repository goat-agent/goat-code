use goat_command::{Command, CommandEffect};

pub struct Effort;

impl Command for Effort {
    fn name(&self) -> &'static str {
        "effort"
    }

    fn description(&self) -> &'static str {
        "set reasoning effort (optional: /effort <level>)"
    }

    fn run(&self, args: &str) -> CommandEffect {
        let args = args.trim();
        if args.is_empty() {
            CommandEffect::OpenEffortPicker
        } else {
            CommandEffect::SelectEffort(args.to_ascii_lowercase())
        }
    }
}
