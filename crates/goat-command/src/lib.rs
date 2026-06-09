pub struct CommandSpec<'a> {
    pub name: &'a str,
    pub description: &'a str,
}

pub enum CommandEffect {
    OpenModelPicker,
    SelectModelNamed(String),
    OpenEffortPicker,
    SelectEffort(String),
    OpenThreadPicker,
    ResumeIndex(usize),
    OpenConfig,
    ShowHelp,
    ClearConversation,
    Submit(String),
    Notice(String),
    Error(String),
    Noop,
}

pub trait Command: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn run(&self, args: &str) -> CommandEffect;
}
