use goat_command::{Command, CommandEffect};

pub struct Model;

impl Command for Model {
    fn name(&self) -> &'static str {
        "model"
    }

    fn description(&self) -> &'static str {
        "switch model"
    }

    fn run(&self, _args: &str) -> CommandEffect {
        CommandEffect::OpenModelPicker
    }
}
