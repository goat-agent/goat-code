use std::fmt::Write as _;

use goat_protocol::SkillInfo;

pub(crate) const SYSTEM_PROMPT: &str = "You are Goat, an expert software engineering assistant. You help users understand, build, and improve software by reading code, running tools, and providing accurate, actionable guidance. When using tools, prefer targeted reads and searches over broad exploration. Always verify your understanding before making changes.";

pub(crate) fn build_system_prompt(skills: &[SkillInfo], instructions: Option<&str>) -> String {
    let mut prompt = String::from(SYSTEM_PROMPT);
    if !skills.is_empty() {
        prompt.push_str(
            "\n\nAvailable skills. Call the Skill tool with a skill's name to load its full instructions before following it:",
        );
        for skill in skills {
            let _ = write!(prompt, "\n- {}: {}", skill.name, skill.description);
        }
    }
    if let Some(content) = instructions {
        let _ = write!(
            prompt,
            "\n\n# Project instructions (AGENTS.md)\n\n{content}"
        );
    }
    prompt
}

pub(crate) fn compose_child_system(base_prompt: &str, instructions: Option<&str>) -> String {
    match instructions {
        None => base_prompt.to_owned(),
        Some(content) => {
            format!("{base_prompt}\n\n# Project instructions (AGENTS.md)\n\n{content}")
        }
    }
}

pub(crate) fn load_skill_infos(cwd: &std::path::Path) -> Vec<SkillInfo> {
    goat_skill::load(cwd)
        .iter()
        .map(|skill| SkillInfo {
            name: skill.name.clone(),
            description: skill.description.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn system_prompt_without_skills_is_base() {
        assert_eq!(super::build_system_prompt(&[], None), super::SYSTEM_PROMPT);
    }

    #[test]
    fn system_prompt_lists_skills() {
        let prompt = super::build_system_prompt(
            &[goat_protocol::SkillInfo {
                name: "demo".to_owned(),
                description: "does the demo".to_owned(),
            }],
            None,
        );
        assert!(prompt.contains("demo"));
        assert!(prompt.contains("does the demo"));
        assert!(prompt.contains("Skill tool"));
    }

    #[test]
    fn system_prompt_includes_project_instructions() {
        let prompt = super::build_system_prompt(&[], Some("always use snake_case"));
        assert!(prompt.contains("always use snake_case"));
        assert!(prompt.contains("Project instructions"));
    }

    #[test]
    fn system_prompt_no_instructions_omits_section() {
        let prompt = super::build_system_prompt(&[], None);
        assert!(!prompt.contains("Project instructions"));
    }
}
