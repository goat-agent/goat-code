use goat_command::{Command, CommandEffect};

pub struct Plan;

impl Command for Plan {
    fn name(&self) -> &'static str {
        "plan"
    }

    fn description(&self) -> &'static str {
        "toggle plan mode (optional: /plan <request>)"
    }

    fn run(&self, args: &str) -> CommandEffect {
        let args = args.trim();
        CommandEffect::PlanMode((!args.is_empty()).then(|| args.to_owned()))
    }
}
