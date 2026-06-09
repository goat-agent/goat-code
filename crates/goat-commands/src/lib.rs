use goat_protocol::SkillInfo;

pub use goat_command::{Command, CommandEffect, CommandSpec};

pub struct CommandRegistry {
    builtins: Vec<Box<dyn Command>>,
    skills: Vec<SkillInfo>,
}

impl CommandRegistry {
    pub fn builtin() -> Self {
        Self {
            builtins: builtin_commands(),
            skills: Vec::new(),
        }
    }

    pub fn set_skills(&mut self, skills: &[SkillInfo]) {
        self.skills = skills.to_vec();
    }

    pub fn resolve(&self, name: &str, args: &str) -> CommandEffect {
        if let Some(command) = self
            .builtins
            .iter()
            .find(|command| command.name() == name || command.aliases().contains(&name))
        {
            return command.run(args);
        }
        if self.skills.iter().any(|skill| skill.name == name) {
            return CommandEffect::Submit(skill_invocation(name, args));
        }
        CommandEffect::Error(format!("unknown command: /{name}"))
    }

    pub fn specs(&self) -> Vec<CommandSpec<'_>> {
        let builtins = self.builtins.iter().map(|command| CommandSpec {
            name: command.name(),
            description: command.description(),
            aliases: command.aliases(),
        });
        let skills = self.skills.iter().map(|skill| CommandSpec {
            name: &skill.name,
            description: &skill.description,
            aliases: &[],
        });
        builtins.chain(skills).collect()
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

fn builtin_commands() -> Vec<Box<dyn Command>> {
    let mut commands = goat_command_settings::all();
    commands.extend(goat_command_conversation::all());
    commands.extend(goat_command_help::all());
    commands.extend(goat_command_app::all());
    commands
}

fn skill_invocation(name: &str, args: &str) -> String {
    let args = args.trim();
    if args.is_empty() {
        format!("Use the \"{name}\" skill.")
    } else {
        format!("Use the \"{name}\" skill.\n\n{args}")
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandEffect, CommandRegistry};
    use goat_protocol::SkillInfo;

    #[test]
    fn builtin_commands_resolve_to_effects() {
        let registry = CommandRegistry::builtin();
        assert!(matches!(
            registry.resolve("model", ""),
            CommandEffect::OpenModelPicker
        ));
        assert!(matches!(
            registry.resolve("config", ""),
            CommandEffect::OpenConfig
        ));
        assert!(matches!(
            registry.resolve("clear", ""),
            CommandEffect::ClearConversation
        ));
        assert!(matches!(
            registry.resolve("help", ""),
            CommandEffect::ShowHelp
        ));
        assert!(matches!(registry.resolve("exit", ""), CommandEffect::Quit));
    }

    #[test]
    fn exit_alias_quit_resolves_to_quit() {
        let registry = CommandRegistry::builtin();
        assert!(matches!(registry.resolve("quit", ""), CommandEffect::Quit));
    }

    #[test]
    fn unknown_command_is_error() {
        assert!(matches!(
            CommandRegistry::builtin().resolve("nope", ""),
            CommandEffect::Error(_)
        ));
    }

    #[test]
    fn skills_resolve_to_submit() {
        let mut registry = CommandRegistry::builtin();
        registry.set_skills(&[SkillInfo {
            name: "demo".to_owned(),
            description: "a demo".to_owned(),
        }]);
        match registry.resolve("demo", "with args") {
            CommandEffect::Submit(text) => {
                assert!(text.contains("demo"));
                assert!(text.contains("with args"));
            }
            _ => panic!("expected submit effect"),
        }
    }

    #[test]
    fn set_skills_replaces_previous() {
        let mut registry = CommandRegistry::builtin();
        registry.set_skills(&[SkillInfo {
            name: "old".to_owned(),
            description: "x".to_owned(),
        }]);
        registry.set_skills(&[SkillInfo {
            name: "new".to_owned(),
            description: "y".to_owned(),
        }]);
        assert!(matches!(
            registry.resolve("old", ""),
            CommandEffect::Error(_)
        ));
        assert!(matches!(
            registry.resolve("new", ""),
            CommandEffect::Submit(_)
        ));
    }

    #[test]
    fn specs_list_builtins_and_skills() {
        let mut registry = CommandRegistry::builtin();
        registry.set_skills(&[SkillInfo {
            name: "demo".to_owned(),
            description: "a demo".to_owned(),
        }]);
        let names: Vec<&str> = registry.specs().into_iter().map(|spec| spec.name).collect();
        assert!(names.contains(&"help"));
        assert!(names.contains(&"exit"));
        assert!(names.contains(&"demo"));
    }

    #[test]
    fn exit_spec_carries_quit_alias() {
        let registry = CommandRegistry::builtin();
        let exit_spec = registry
            .specs()
            .into_iter()
            .find(|s| s.name == "exit")
            .expect("exit spec missing");
        assert!(exit_spec.aliases.contains(&"quit"));
    }
}
