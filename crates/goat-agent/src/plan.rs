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

const ENTER_INJECT: &str = "Plan mode is on. Investigate the request above, write the plan to the plan file, and call ProposePlan when it is ready for the user to approve.";

pub(crate) fn enter_plan_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: ENTER_PLAN_TOOL_NAME.to_owned(),
        description: "Switch into plan mode to design an approach before changing anything. Call this proactively before a non-trivial implementation task — a new feature, a change to existing behavior, an architectural choice, work spanning several files, or anything where the user's preference matters — so the user can approve the approach before code is written. Skip it for trivial edits, single obvious fixes, or pure research. Before calling, say in one line why you are switching to planning. Takes no input.".to_owned(),
        input_schema: serde_json::json!({ "type": "object", "properties": {} }),
    }
}

pub(crate) fn propose_plan_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: PROPOSE_PLAN_TOOL_NAME.to_owned(),
        description: "Present the finished plan to the user for approval and leave plan mode. Reads the plan from the plan file you wrote — takes no input. Call it only after the plan file is complete and you have done any needed investigation; do not announce a plan in prose instead. If the user requests changes, you keep planning with their feedback; if they approve, implementation begins.".to_owned(),
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
    let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    if content.trim().is_empty() {
        return Err(
            "the plan file is empty — write the plan to it before proposing it for approval"
                .to_owned(),
        );
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
        Ok(PlanDecision::Approve) => {
            if let Some(cell) = env.transition {
                let inject = format!(
                    "The plan at {} was approved. Implement it now — read that file for the full plan, then make the changes and verify them.",
                    path.display()
                );
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
        "\n\n# Plan mode\n\nYou are in plan mode: a no-change regime for designing an approach the user approves before any edits happen. The only file you may create or modify is the plan file:\n\n  {plan_path}\n\nUse the Write and Edit tools with that exact path to create and update the plan as you investigate — those tools are permitted to write this one file even though it sits outside the workspace. Do NOT write it with shell redirection (`>`, `>>`, `tee`): the shell is read-only here and will refuse. The plan must be self-contained — state the context (why), the concrete changes (what, by file), and how to verify them — so it stays useful after the conversation is compacted.\n\n{shell}\n\nWorkflow: investigate (delegate explore and architect sub-agents for breadth and design), use the Ask tool when the user's judgment is needed, write the plan to the file with Write/Edit, then call ProposePlan to present it for approval. Never claim a plan is ready in prose — always call ProposePlan. If the user asks you to implement immediately, tell them plan mode is on and either present the plan with ProposePlan or note they can turn plan mode off. If the plan file already has content from earlier in this session, continue or revise it rather than starting over."
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
}
