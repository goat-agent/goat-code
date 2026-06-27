use goat_command::{
    Command, CommandEffect, CommandInvocation, CommandShape, ParameterSpec, ParameterValue,
};

pub struct Plan;

impl Command for Plan {
    fn name(&self) -> &'static str {
        "plan"
    }

    fn description(&self) -> &'static str {
        "toggle plan mode"
    }

    fn shape(&self) -> CommandShape {
        CommandShape::Parameters(vec![ParameterSpec {
            name: "request".to_owned(),
            description: "optional planning request".to_owned(),
            required: false,
            value: ParameterValue::TextTail,
        }])
    }

    fn run(&self, invocation: CommandInvocation) -> CommandEffect {
        CommandEffect::PlanMode(invocation.text("request").map(str::to_owned))
    }
}
