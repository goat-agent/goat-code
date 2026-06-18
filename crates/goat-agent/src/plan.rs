use std::path::PathBuf;

use goat_protocol::{Event, Mode, PlanDecision, ToolCallId, ToolDisplay};
use goat_provider::ToolDefinition;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::{Ctx, LoopEnv, Run};

pub(crate) const ENTER_PLAN_TOOL_NAME: &str = "EnterPlanMode";
pub(crate) const PROPOSE_PLAN_TOOL_NAME: &str = "ProposePlan";

pub(crate) const PLAN_REGISTRY_TOOLS: [&str; 5] = ["Read", "Grep", "Glob", "Skill", "WebFetch"];

fn plan_names(plan_shell: bool, with_write: bool) -> Vec<String> {
    let mut names: Vec<String> = PLAN_REGISTRY_TOOLS
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    if plan_shell {
        names.push("Bash".to_owned());
    }
    if with_write {
        names.push("Write".to_owned());
        names.push("Edit".to_owned());
    }
    names
}

pub(crate) fn plan_selection(plan_shell: bool) -> crate::agent::ToolSelection {
    crate::agent::ToolSelection::Only(plan_names(plan_shell, true))
}

pub(crate) fn plan_child_selection(plan_shell: bool) -> crate::agent::ToolSelection {
    crate::agent::ToolSelection::Only(plan_names(plan_shell, false))
}

pub(crate) struct Transition {
    pub(crate) mode: Mode,
    pub(crate) inject: String,
}

pub(crate) type TransitionCell = std::sync::Mutex<Option<Transition>>;

const ENTER_INJECT: &str = "Plan mode is on. Prepare an approval contract before making project changes: investigate read-only evidence, ask only for material user judgment, update the plan file, and call ProposePlan only when the plan is mature enough for approval.";

pub(crate) fn enter_plan_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: ENTER_PLAN_TOOL_NAME.to_owned(),
        description: "Switch into plan mode to prepare an approval contract before meaningful project changes. Call this proactively for work where goal, approach, validation, user preference, architecture, cross-file scope, persistence, security, public behavior, or reversibility matters. Skip it for trivial edits, single obvious fixes, and pure research. Before calling, say in one line why planning is useful here. Takes no input.".to_owned(),
        input_schema: serde_json::json!({ "type": "object", "properties": {} }),
    }
}

pub(crate) fn propose_plan_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: PROPOSE_PLAN_TOOL_NAME.to_owned(),
        description: "Present the mature plan from the plan file as an approval contract and leave plan mode. Takes no input. Call it only after needed investigation, material questions, and plan-file updates are complete; do not treat a non-empty plan file as ready by itself, and do not announce approval plans only in prose. If the user requests changes, keep planning with their feedback; if they approve, implement within the approved scope.".to_owned(),
        input_schema: serde_json::json!({ "type": "object", "properties": {} }),
    }
}

pub(crate) fn enter_plan_display() -> ToolDisplay {
    ToolDisplay::primary("enter plan mode")
}

pub(crate) fn propose_plan_display() -> ToolDisplay {
    ToolDisplay::primary("propose plan")
}

pub(crate) fn run_enter_plan(env: &LoopEnv<'_>) -> Result<String, String> {
    let Some(cell) = env.transition else {
        return Err("plan mode cannot be entered from here".to_owned());
    };
    let mut guard = cell
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.is_some() {
        return Err("a mode switch is already pending".to_owned());
    }
    *guard = Some(Transition {
        mode: Mode::Plan,
        inject: ENTER_INJECT.to_owned(),
    });
    Ok("Entering plan mode at the next turn.".to_owned())
}

pub(crate) async fn run_propose_plan(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    env: &LoopEnv<'_>,
    call_id: ToolCallId,
    token: &CancellationToken,
) -> Result<String, String> {
    let Some(path) = env.plan_path.clone() else {
        return Err("no plan file is set for this session".to_owned());
    };
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(format!(
                "could not read the plan file at {}: {err}",
                path.display()
            ));
        }
    };
    if content.trim().is_empty() {
        return Err(
            "the plan file is empty — write the plan to it before proposing it for approval"
                .to_owned(),
        );
    }
    if let Some(found) = find_placeholder(&content) {
        return Err(format!(
            "the plan still contains a placeholder ({found}) — fill it in before proposing the plan for approval"
        ));
    }
    let (tx, rx) = oneshot::channel::<PlanDecision>();
    ctx.plans.lock().await.insert(call_id, tx);
    let _ = ctx
        .events
        .send(Event::PlanProposed {
            id: run.id,
            call: call_id,
            plan: content,
            path: path.display().to_string(),
        })
        .await;
    let decision = tokio::select! {
        biased;
        () = token.cancelled() => {
            ctx.plans.lock().await.remove(&call_id);
            let _ = ctx
                .events
                .send(Event::PlanDismissed { id: run.id, call: call_id })
                .await;
            return Err("interrupted".to_owned());
        }
        res = rx => res,
    };
    match decision {
        Ok(PlanDecision::Approve {}) => {
            if let Some(cell) = env.transition {
                let inject = approved_plan_inject(&path);
                *cell
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Transition {
                    mode: Mode::Normal,
                    inject,
                });
            }
            Ok("The plan was approved; implementation begins now.".to_owned())
        }
        Ok(PlanDecision::Reject { feedback }) => Ok(format!(
            "The user did not approve the plan and asked for changes. Revise the plan file accordingly, then call ProposePlan again. Their feedback:\n\n{feedback}"
        )),
        Err(_) => Err("the approval channel closed".to_owned()),
    }
}

fn approved_plan_inject(path: &std::path::Path) -> String {
    format!(
        "The plan at {} was approved. Implement it now: read the approved plan first, follow its goal, scope, approach, and validation strategy, and keep safe local implementation discretion. If implementation requires material deviation such as scope expansion, user-visible behavior change, public API/protocol/schema/persistence/auth/security impact, a new dependency, changed validation, dropped verification, contradiction of explicit user preference, or a false core assumption, stop to ask, explicitly exclude it, or replan. After changes, run the planned verification when practical and report what changed, verification results, deviations, anything not done, and remaining risks.",
        path.display()
    )
}

pub(crate) fn find_placeholder(content: &str) -> Option<&'static str> {
    const TOKENS: [&str; 5] = ["TODO", "TBD", "FIXME", "<...>", "REPLACE_ME"];
    let upper = content.to_uppercase();
    TOKENS.into_iter().find(|token| upper.contains(*token))
}

pub(crate) fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = true;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let trimmed = slug.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        "plan".to_owned()
    } else {
        trimmed
    }
}

pub(crate) fn resolve_plan_path(thread_id: Option<i64>, slug_source: &str) -> Option<PathBuf> {
    let dir = goat_config::plans_dir()?;
    let id = thread_id.unwrap_or(0);
    if let Some(existing) = recover_plan_path(&dir, id) {
        return Some(existing);
    }
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::warn!(%err, "could not create plans directory");
        return None;
    }
    let file = format!("{}-{id}.md", slugify(slug_source));
    let path = dir.join(file);
    Some(path.canonicalize().unwrap_or(path))
}

fn recover_plan_path(dir: &std::path::Path, thread_id: i64) -> Option<PathBuf> {
    let suffix = format!("-{thread_id}.md");
    let entries = std::fs::read_dir(dir).ok()?;
    let mut matches: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
        })
        .map(|path| {
            let mtime = path
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (mtime, path)
        })
        .collect();
    if matches.len() > 1 {
        tracing::warn!(
            thread_id,
            count = matches.len(),
            "multiple plan files match thread; using newest"
        );
    }
    matches.sort_by_key(|(mtime, _)| *mtime);
    matches
        .pop()
        .map(|(_, path)| path.canonicalize().unwrap_or(path))
}

pub(crate) fn plan_segment(plan_path: &str, shell_available: bool) -> String {
    let shell = if shell_available {
        "Shell commands run under a read-only sandbox: investigation works (git log, grep, reading config) but writing anything outside scratch space is blocked, so building and running tests is not possible here — describe verification in the plan and do it after approval."
    } else {
        "Shell commands are unavailable in plan mode on this machine (no sandbox backend), so rely on Read, Grep, Glob, WebSearch, and WebFetch for investigation."
    };
    format!(
        "\n\n# Plan mode\n\nYou are in plan mode: an execution-control regime for preparing an approval contract before meaningful project changes. Planning is before commitment, not before thought: use read-only investigation, search, file inspection, and focused analysis to reduce goal error, approach error, and validation error before proposing implementation. The only file you may create or modify is the plan file:\n\n  {plan_path}\n\nUse Write and Edit with that exact path to update the plan as the planning state matures. Those tools may write this one file even though it sits outside the workspace. Do NOT write it with shell redirection (`>`, `>>`, `tee`): the shell is read-only here and will refuse. Treat the plan file as durable shared state, but keep the visible plan a tight account of decisions, not a raw research log.\n\n{shell}\n\nThe one test for everything you do here — investigate, ask, or write to the plan — is whether it is needed to reach a correct plan: one the agent can implement after approval without mistakes and matching intent. If it is needed, do it; otherwise it is noise, leave it out.\n\nInvestigate first, before asking the user anything. When you need a piece of information, route it: a descriptive fact about the code or system (what exists, how it works) you discover yourself with Read, Grep, Glob, and search — never ask the user what you can inspect. A decision that is costly or hard to reverse you ask the user. A decision that is trivial, reversible, and has one obviously dominant reading you settle yourself and fold into the plan rather than asking.\n\nEvidence comes before speculation and before user questions when you can inspect the facts yourself. Ask only for material user judgment about intent, priority, preference, scope, or trade-offs. Ask dependent or pivotal questions one at a time so each answer shapes the next, and pair each with a one-line reason it matters plus your recommended default when the evidence points one way (for pure-taste choices give options without steering). Batch only genuinely independent, trivial questions together, as bare options with no explanation. When an answer is shallow on something that matters, press one step deeper — demand an example, surface the hidden assumption, or force a boundary — with depth proportional to that decision's risk and blast radius; do not interrogate trivia. Stop asking once not knowing a thing would no longer break the implementation.\n\nPlan adaptively. Keep trivial, obvious, reversible work very light. For ambiguous, cross-cutting, user-visible, persistence, security, public API, architecture, or hard-to-verify work, investigate more deeply and design stronger validation. For product, UX, creative, or greenfield work, first shape the problem — a few options, trade-offs, and a recommended direction — before turning the chosen direction into an implementation plan.\n\nWrite the plan as a scannable set of headed sections, but only the sections this work actually needs — never pad with empty or boilerplate headings. Within each section write woven prose that states what will be built and why it was decided that way, not a flat checklist of decisions and not a narration of your planning process. Trivial settled decisions belong inside that prose as a clause, not as their own callout. Detail is welcome where it prevents an implementation mistake; process narration, unresolved open questions, and hedging are noise — resolve or scope them out instead of leaving them in.\n\nBefore proposing, review your own plan. The mechanical pass is reliable: scan for leftover placeholders and inconsistent names against what the plan itself defines, and confirm every stated requirement maps to something in the plan. For genuine quality — is this the right approach, did it miss an edge case or an alternative — delegate to read-only critic agents, each given the plan and one perspective drawn from the work (architecture, quality, or security for code; product or problem-framing for a new project; trade-offs or alternatives for design), as many in parallel as the risk warrants. A critic only flags; it cannot and must not edit the plan. Bring its findings into the conversation, resolve them with the user, then you apply the agreed changes — never silently fold a critic's opinion into the plan as if it were settled. Delegation is optional and purpose-driven, scaled to consequence; do not delegate as ritual.\n\nThe plan constrains material choices, not every local implementation detail. Material deviation triggers worth honoring during implementation: scope expansion, user-visible behavior change, public API/protocol/schema/persistence/auth/security impact, new dependency, substantially touching unrelated systems, changed validation strategy, dropped planned verification, contradiction of explicit user preference, or a false core assumption. Safe local tactical choices can remain agent discretion when validation is clear.\n\nWhen the plan is mature enough for approval, write or revise the plan file and call ProposePlan. Do not treat a non-empty plan file as ready by itself. Do not claim an approval plan is ready only in prose. If the user signals they want to stop planning and proceed (\"just do it\"), call ProposePlan with the best plan you have rather than continuing to investigate. If the user asks you to implement while plan mode is on, either call ProposePlan or explain the remaining blocker to approval. If the plan file already has content from earlier in this session, continue or revise it rather than starting over."
    )
}

#[cfg(test)]
mod tests {
    use super::slugify;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Fix the auth bug"), "fix-the-auth-bug");
    }

    #[test]
    fn slugify_strips_symbols_and_unicode() {
        assert_eq!(slugify("  버그!! fix??  "), "fix");
        assert_eq!(slugify("a/b\\c"), "a-b-c");
    }

    #[test]
    fn slugify_empty_falls_back() {
        assert_eq!(slugify("***"), "plan");
        assert_eq!(slugify(""), "plan");
    }

    #[test]
    fn top_gets_write_child_does_not() {
        let top = super::plan_selection(true);
        assert!(top.allows("Write"));
        assert!(top.allows("Edit"));
        assert!(top.allows("Bash"));
        let child = super::plan_child_selection(true);
        assert!(!child.allows("Write"));
        assert!(!child.allows("Edit"));
        assert!(child.allows("Read"));
        assert!(child.allows("Bash"));
    }

    #[test]
    fn no_shell_excludes_bash() {
        let top = super::plan_selection(false);
        assert!(!top.allows("Bash"));
        assert!(top.allows("Write"));
    }

    #[test]
    fn slugify_caps_length() {
        let long = "word ".repeat(40);
        assert!(slugify(&long).len() <= 48);
    }

    #[test]
    fn plan_segment_contains_execution_control_guidance() {
        let prompt = super::plan_segment("/tmp/plan.md", true);
        for expected in [
            "execution-control regime",
            "goal error, approach error, and validation error",
            "only file you may create or modify is the plan file",
            "Plan adaptively",
            "Evidence comes before speculation",
            "Ask only for material user judgment",
            "Delegation is optional",
            "product, UX, creative, or greenfield work",
            "Material deviation triggers",
            "Do not treat a non-empty plan file as ready by itself",
        ] {
            assert!(prompt.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn plan_segment_contains_v2_design_guidance() {
        let prompt = super::plan_segment("/tmp/plan.md", true);
        for expected in [
            "Investigate first, before asking",
            "descriptive fact about the code",
            "one at a time",
            "depth proportional to that decision's risk",
            "no longer break the implementation",
            "only the sections this work actually needs",
            "read-only critic agents",
            "A critic only flags",
            "just do it",
        ] {
            assert!(prompt.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn find_placeholder_flags_tokens_and_passes_clean() {
        assert_eq!(super::find_placeholder("## Goal\nfix the bug"), None);
        assert_eq!(super::find_placeholder("approach: TODO"), Some("TODO"));
        assert_eq!(super::find_placeholder("steps: tbd later"), Some("TBD"));
        assert_eq!(super::find_placeholder("name: <...>"), Some("<...>"));
        assert_eq!(super::find_placeholder("see notes, etc. and so on"), None);
    }

    #[test]
    fn approval_handoff_contains_change_control_guidance() {
        let path = std::path::Path::new("/tmp/approved.md");
        let inject = super::approved_plan_inject(path);
        for expected in [
            "read the approved plan first",
            "safe local implementation discretion",
            "material deviation",
            "scope expansion",
            "changed validation",
            "run the planned verification",
            "report what changed",
        ] {
            assert!(inject.contains(expected), "missing {expected}");
        }
    }
}
