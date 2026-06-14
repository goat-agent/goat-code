use goat_command::{
    Command, CommandEffect, CommandInvocation, CommandShape, ParameterSpec, ParameterValue,
};

pub struct Resume;

impl Command for Resume {
    fn name(&self) -> &'static str {
        "resume"
    }

    fn description(&self) -> &'static str {
        "resume a past conversation"
    }

    fn shape(&self) -> CommandShape {
        CommandShape::Parameters(vec![ParameterSpec {
            name: "n".to_owned(),
            description: "conversation number".to_owned(),
            required: false,
            value: ParameterValue::Integer,
        }])
    }

    fn run(&self, invocation: CommandInvocation) -> CommandEffect {
        if let Some(n) = invocation.integer("n") {
            match usize::try_from(n) {
                Ok(n) if n >= 1 => CommandEffect::ResumeIndex(n - 1),
                _ => CommandEffect::Error("resume index must be at least 1".to_owned()),
            }
        } else {
            CommandEffect::OpenThreadPicker
        }
    }
}
