use std::fmt::Write as _;

use goat_protocol::SkillInfo;

pub(crate) const SYSTEM_PROMPT: &str = concat!(
    "You are Goat, a concise software-engineering agent working in a terminal workspace.\n\n",
    "- Follow the user's request and project instructions; ask or explain constraints when needed.\n",
    "- Ground claims in files, tool output, or cited sources; do not invent code, paths, results, or citations.\n",
    "- Work efficiently with targeted inspection first; delegate only when it saves context or time.\n",
    "- Before editing, understand the relevant code, preserve unrelated changes, and make the smallest maintainable change.\n",
    "- Verify when practical, then respond succinctly with the result, verification, and any remaining risks or next steps."
);

pub(crate) fn build_system_prompt(skills: &[SkillInfo], instructions: Option<&str>) -> String {
    let mut prompt = String::from(SYSTEM_PROMPT);
    if !skills.is_empty() {
        prompt.push_str("\n\n# Skills\n\nLoad a skill with the Skill tool before following it:");
        for skill in skills {
            let _ = write!(prompt, "\n- {}: {}", skill.name, skill.description);
        }
    }
    if let Some(content) = instructions {
        let _ = write!(prompt, "\n\n{content}");
    }
    prompt
}

pub(crate) fn compose_child_system(base_prompt: &str, instructions: Option<&str>) -> String {
    match instructions {
        None => base_prompt.to_owned(),
        Some(content) => format!("{base_prompt}\n\n{content}"),
    }
}

pub(crate) fn load_skill_infos(cwd: &std::path::Path) -> Vec<SkillInfo> {
    goat_skill::load(cwd).iter().map(SkillInfo::from).collect()
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
                command: None,
            }],
            None,
        );
        assert!(prompt.contains("# Skills"));
        assert!(prompt.contains("demo"));
        assert!(prompt.contains("does the demo"));
        assert!(prompt.contains("Skill tool"));
    }

    #[test]
    fn system_prompt_includes_project_instructions() {
        let prompt = super::build_system_prompt(&[], Some("always use snake_case"));
        assert!(prompt.contains("always use snake_case"));
    }

    #[test]
    fn system_prompt_no_instructions_omits_section() {
        let prompt = super::build_system_prompt(&[], None);
        assert!(!prompt.contains("Project instructions"));
    }

    #[test]
    fn system_prompt_appends_project_instructions_verbatim() {
        let heading = "# Project instructions (repo/AGENTS.md)";
        let instructions = format!("{heading}\n\nalways use snake_case");
        let prompt = super::build_system_prompt(&[], Some(&instructions));
        assert_eq!(prompt.matches(heading).count(), 1);
        assert!(prompt.ends_with(&instructions));
        assert!(!prompt.contains("# Project instructions (AGENTS.md)\n\n# Project instructions"));
    }

    #[test]
    fn child_system_appends_project_instructions_verbatim() {
        let heading = "# Project instructions (x)";
        let instructions = format!("{heading}\n\nrule");
        let prompt = super::compose_child_system("child base", Some(&instructions));
        assert_eq!(prompt.matches(heading).count(), 1);
        assert!(prompt.starts_with("child base"));
        assert!(prompt.ends_with(&instructions));
        assert!(!prompt.contains("# Project instructions (AGENTS.md)\n\n# Project instructions"));
    }

    #[test]
    fn system_prompt_orders_sections() {
        let prompt = super::build_system_prompt(
            &[goat_protocol::SkillInfo {
                name: "demo".to_owned(),
                description: "does the demo".to_owned(),
            }],
            Some("# Project instructions (repo/AGENTS.md)\n\nrule"),
        );
        let base = prompt.find(super::SYSTEM_PROMPT).unwrap();
        let skills = prompt.find("# Skills").unwrap();
        let instructions = prompt.find("# Project instructions").unwrap();
        assert!(base < skills);
        assert!(skills < instructions);
    }
}
