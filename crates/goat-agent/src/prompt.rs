use std::fmt::Write as _;

use goat_protocol::SkillInfo;
use goat_provider::ContentBlock;

pub(crate) const PRINCIPLES: &str = concat!(
    "You are Goat, a software-engineering agent working in a terminal workspace. ",
    "You act through tools and speak to the user through a transcript.\n\n",
    "- Do what the request asks and respect project conventions; surface blocking constraints or ambiguity instead of guessing, and ask the user when a choice is material and you cannot settle it from the workspace.\n",
    "- Ground every claim in files, tool output, or cited sources; never invent code, paths, results, or citations, and say so when you are unsure.\n",
    "- Prefer targeted inspection over broad reading; understand code before changing it, keep changes minimal and consistent with the surrounding code, and leave unrelated lines untouched.\n",
    "- Verify your work when a check is available and confirm it actually holds before claiming it is done; then report plainly what you did, how you know it holds, and any remaining risks or next steps.\n",
    "- Reply to the user in their language, but keep code, identifiers, paths, commands, tool arguments, and quoted excerpts verbatim; write text stored in the repository (commit messages, comments, PR descriptions) in the project's prevailing language."
);

pub(crate) const LANGUAGE_REMINDER: &str = "[Reminder: write your prose to the user in the language they used in their request. Keep code, identifiers, file paths, shell commands, tool arguments, and quoted file or output excerpts exactly as they are. Text stored in the repository stays in the project's prevailing language.]";

pub(crate) fn language_anchor_block() -> ContentBlock {
    ContentBlock::Text {
        text: LANGUAGE_REMINDER.to_owned(),
    }
}

pub(crate) fn append_language_anchor(
    mut content: Vec<ContentBlock>,
    is_top: bool,
) -> Vec<ContentBlock> {
    if is_top {
        content.push(language_anchor_block());
    }
    content
}

fn env_segment(cwd: &std::path::Path, os: &str) -> String {
    format!("\n\n# Environment\n\n- cwd: {}\n- os: {os}", cwd.display())
}

pub(crate) fn build_system_prompt(
    cwd: &std::path::Path,
    skills: &[SkillInfo],
    instructions: Option<&str>,
) -> String {
    let mut prompt = String::from(PRINCIPLES);
    prompt.push_str(&env_segment(cwd, std::env::consts::OS));
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
    use std::path::Path;

    #[test]
    fn system_prompt_starts_with_principles_and_lists_environment() {
        let prompt = super::build_system_prompt(Path::new("/work/project"), &[], None);
        assert!(prompt.starts_with(super::PRINCIPLES));
        assert!(prompt.contains("# Environment"));
        assert!(prompt.contains("cwd: /work/project"));
        assert!(prompt.contains(&format!("os: {}", std::env::consts::OS)));
        assert!(!prompt.contains("# Skills"));
    }

    #[test]
    fn env_block_lists_cwd_and_os() {
        let segment = super::env_segment(Path::new("/tmp/here"), "linux");
        assert!(segment.contains("# Environment"));
        assert!(segment.contains("- cwd: /tmp/here"));
        assert!(segment.contains("- os: linux"));
    }

    #[test]
    fn system_prompt_lists_skills() {
        let prompt = super::build_system_prompt(
            Path::new("/work"),
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
        let prompt =
            super::build_system_prompt(Path::new("/work"), &[], Some("always use snake_case"));
        assert!(prompt.contains("always use snake_case"));
    }

    #[test]
    fn system_prompt_no_instructions_omits_section() {
        let prompt = super::build_system_prompt(Path::new("/work"), &[], None);
        assert!(!prompt.contains("Project instructions"));
    }

    #[test]
    fn system_prompt_appends_project_instructions_verbatim() {
        let heading = "# Project instructions (repo/AGENTS.md)";
        let instructions = format!("{heading}\n\nalways use snake_case");
        let prompt = super::build_system_prompt(Path::new("/work"), &[], Some(&instructions));
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
            Path::new("/work"),
            &[goat_protocol::SkillInfo {
                name: "demo".to_owned(),
                description: "does the demo".to_owned(),
                command: None,
            }],
            Some("# Project instructions (repo/AGENTS.md)\n\nrule"),
        );
        let base = prompt.find(super::PRINCIPLES).unwrap();
        let env = prompt.find("# Environment").unwrap();
        let skills = prompt.find("# Skills").unwrap();
        let instructions = prompt.find("# Project instructions").unwrap();
        assert!(base < env);
        assert!(env < skills);
        assert!(skills < instructions);
    }

    #[test]
    fn system_prompt_carries_language_policy() {
        let prompt = super::build_system_prompt(Path::new("/work"), &[], None);
        assert!(prompt.contains("Reply to the user in their language"));
        assert!(prompt.contains("keep code, identifiers, paths, commands, tool arguments"));
        assert!(prompt.contains("project's prevailing language"));
    }

    #[test]
    fn language_anchor_appends_only_for_top_run() {
        use goat_provider::ContentBlock;
        let base = vec![ContentBlock::text_result("call_1", "ok", false)];
        let top = super::append_language_anchor(base.clone(), true);
        assert_eq!(top.len(), 2);
        assert!(matches!(
            top.last(),
            Some(ContentBlock::Text { text }) if text == super::LANGUAGE_REMINDER
        ));
        let child = super::append_language_anchor(base, false);
        assert_eq!(child.len(), 1);
        assert!(matches!(
            child.last(),
            Some(ContentBlock::ToolResult { .. })
        ));
    }
}
