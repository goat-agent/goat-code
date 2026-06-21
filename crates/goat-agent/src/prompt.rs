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

fn env_segment(cwd: &std::path::Path, os: &str, date: &str) -> String {
    format!(
        "\n\n# Environment\n\n- date: {date}\n- cwd: {}\n- os: {os}",
        cwd.display()
    )
}

pub(crate) fn current_utc_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let (year, month, day) = civil_date_from_unix_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn civil_date_from_unix_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

pub(crate) fn build_system_prompt(
    cwd: &std::path::Path,
    skills: &[SkillInfo],
    instructions: Option<&str>,
    date: &str,
) -> String {
    let mut prompt = String::from(PRINCIPLES);
    prompt.push_str(&env_segment(cwd, std::env::consts::OS, date));
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
        let prompt =
            super::build_system_prompt(Path::new("/work/project"), &[], None, "2025-01-15");
        assert!(prompt.starts_with(super::PRINCIPLES));
        assert!(prompt.contains("# Environment"));
        assert!(prompt.contains("cwd: /work/project"));
        assert!(prompt.contains(&format!("os: {}", std::env::consts::OS)));
        assert!(!prompt.contains("# Skills"));
    }

    #[test]
    fn env_block_lists_date_cwd_and_os() {
        let segment = super::env_segment(Path::new("/tmp/here"), "linux", "2025-01-15");
        assert!(segment.contains("# Environment"));
        assert!(segment.contains("- date: 2025-01-15"));
        assert!(segment.contains("- cwd: /tmp/here"));
        assert!(segment.contains("- os: linux"));
    }

    #[test]
    fn current_utc_date_is_iso_formatted() {
        let date = super::current_utc_date();
        let bytes = date.as_bytes();
        assert_eq!(date.len(), 10);
        assert_eq!(bytes[4], b'-');
        assert_eq!(bytes[7], b'-');
        assert!(date[0..4].chars().all(|c| c.is_ascii_digit()));
        assert!(date[5..7].chars().all(|c| c.is_ascii_digit()));
        assert!(date[8..10].chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn civil_date_matches_known_unix_days() {
        assert_eq!(super::civil_date_from_unix_days(0), (1970, 1, 1));
        assert_eq!(super::civil_date_from_unix_days(19_723), (2024, 1, 1));
        assert_eq!(super::civil_date_from_unix_days(20_134), (2025, 2, 15));
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
            "2025-01-15",
        );
        assert!(prompt.contains("# Skills"));
        assert!(prompt.contains("demo"));
        assert!(prompt.contains("does the demo"));
        assert!(prompt.contains("Skill tool"));
    }

    #[test]
    fn system_prompt_includes_project_instructions() {
        let prompt = super::build_system_prompt(
            Path::new("/work"),
            &[],
            Some("always use snake_case"),
            "2025-01-15",
        );
        assert!(prompt.contains("always use snake_case"));
    }

    #[test]
    fn system_prompt_no_instructions_omits_section() {
        let prompt = super::build_system_prompt(Path::new("/work"), &[], None, "2025-01-15");
        assert!(!prompt.contains("Project instructions"));
    }

    #[test]
    fn system_prompt_appends_project_instructions_verbatim() {
        let heading = "# Project instructions (repo/AGENTS.md)";
        let instructions = format!("{heading}\n\nalways use snake_case");
        let prompt =
            super::build_system_prompt(Path::new("/work"), &[], Some(&instructions), "2025-01-15");
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
            "2025-01-15",
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
        let prompt = super::build_system_prompt(Path::new("/work"), &[], None, "2025-01-15");
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
