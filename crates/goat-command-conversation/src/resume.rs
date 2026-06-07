use goat_command::{Command, CommandEffect};

pub struct Resume;

impl Command for Resume {
    fn name(&self) -> &'static str {
        "resume"
    }

    fn description(&self) -> &'static str {
        "resume a past conversation (optional: /resume <n>)"
    }

    fn run(&self, args: &str) -> CommandEffect {
        let args = args.trim();
        if args.is_empty() {
            return CommandEffect::OpenThreadPicker;
        }
        match args.parse::<usize>() {
            Ok(n) if n >= 1 => CommandEffect::ResumeIndex(n - 1),
            _ => CommandEffect::Error("usage: /resume <n>".to_owned()),
        }
    }
}
