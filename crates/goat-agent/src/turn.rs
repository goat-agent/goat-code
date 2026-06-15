use std::path::PathBuf;

use goat_protocol::{Event, Mode, ModelTarget, Op, TaskId};
use goat_provider::{Message, MessageRole, Provider, ToolDefinition};
use goat_store::Store;
use goat_tool::{SandboxPolicy, ToolContext, ToolError};
use goat_tools::ToolRegistry;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    Ctx, Flow, Run,
    accounts::provider_for,
    compaction::ContextTracker,
    conversation::Conversation,
    persist::{
        effort_string, ensure_thread, finalize_turn, init_db_turn, now_ms, persist_shell_message,
        thread_title,
    },
    plan,
    prompt::build_system_prompt,
    rounds::{LoopOutcome, core_loop},
    shell,
    threads::resolve_thread_cwd,
    tools_exec::{TransitionTool, build_tool_defs},
};

fn top_regime(
    ctx: &Ctx<'_>,
    provider: &dyn Provider,
    mode: Mode,
) -> (Vec<ToolDefinition>, SandboxPolicy) {
    match mode {
        Mode::Normal => (
            build_tool_defs(ctx, provider, None, true, TransitionTool::Enter),
            SandboxPolicy::Full,
        ),
        Mode::Plan => {
            let selection = plan::plan_selection(ctx.plan_shell);
            (
                build_tool_defs(
                    ctx,
                    provider,
                    Some(&selection),
                    true,
                    TransitionTool::Propose,
                ),
                SandboxPolicy::ReadOnly { network: false },
            )
        }
    }
}

async fn apply_set_mode(
    ctx: &Ctx<'_>,
    requested: Mode,
    thread_id: Option<i64>,
    mode: &mut Mode,
    plan_path: &mut Option<PathBuf>,
) {
    *mode = requested;
    if requested == Mode::Normal {
        *plan_path = None;
    }
    if let Some(tid) = thread_id
        && let Err(err) = ctx
            .store
            .update_thread_mode(tid, crate::mode_string(requested), now_ms())
            .await
    {
        tracing::warn!(%err, "failed to persist thread mode");
    }
    let _ = ctx
        .events
        .send(Event::ModeChanged {
            mode: requested,
            plan_path: plan_path.as_ref().map(|path| path.display().to_string()),
        })
        .await;
}

const SHELL_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(10);
pub(crate) const SHELL_INTERRUPTED: &str = "[interrupted]";

enum ShellEnd {
    Done(String),
    Interrupted,
    Shutdown,
}

async fn run_shell_command(tools: &ToolRegistry, command: &str, cwd: &std::path::Path) -> String {
    let mut tool_ctx = match ToolContext::new(cwd) {
        Ok(tool_ctx) => tool_ctx,
        Err(err) => return err.to_string(),
    };
    tool_ctx.bash_timeout = SHELL_TIMEOUT;
    let Some(tool) = tools.get("Bash") else {
        return "shell tool unavailable".to_owned();
    };
    let input = serde_json::json!({ "command": command }).to_string();
    match tool.run(&input, &tool_ctx).await {
        Ok(output) => output.as_text().unwrap_or_default().to_owned(),
        Err(ToolError::Timeout { .. }) => {
            format!("[timed out after {}m]", SHELL_TIMEOUT.as_secs() / 60)
        }
        Err(err) => err.to_string(),
    }
}

pub(crate) enum TurnEnd {
    Done,
    Interrupted,
    Failed(String),
    Shutdown,
}

pub(crate) async fn emit_task_error(ctx: &Ctx<'_>, id: TaskId, message: String) {
    let _ = ctx
        .events
        .send(Event::Error {
            id: Some(id),
            message,
        })
        .await;
    let _ = ctx
        .events
        .send(Event::TaskDone {
            id,
            interrupted: true,
        })
        .await;
}

pub(crate) async fn handle_idle_op(
    op: Op,
    store: &Store,
    thread_id: Option<i64>,
    target: &mut Option<ModelTarget>,
    events: &mpsc::Sender<Event>,
) {
    match op {
        Op::SelectModel { target: chosen } => {
            if let Some(tid) = thread_id
                && let Err(err) = store
                    .update_thread_model(
                        tid,
                        chosen.provider.clone(),
                        chosen.model.clone(),
                        chosen.account.clone(),
                        effort_string(chosen.effort),
                        now_ms(),
                    )
                    .await
            {
                tracing::warn!(%err, "failed to update thread model");
            }
            *target = Some(chosen.clone());
            let _ = events.send(Event::ModelSelected { target: chosen }).await;
        }
        Op::RenameThread { title } => {
            crate::threads::handle_rename(store, thread_id, title, events).await;
        }
        Op::ListThreads => {
            crate::threads::handle_list_threads(store, events).await;
        }
        _ => {}
    }
}

enum TurnFlow {
    Idle,
    Done(std::collections::VecDeque<(TaskId, String)>),
    Shutdown,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_turn(
    ctx: &Ctx<'_>,
    id: TaskId,
    text: String,
    target: &mut Option<ModelTarget>,
    conversation: &mut Conversation,
    tracker: &mut ContextTracker,
    thread_id: &mut Option<i64>,
    mode: &mut Mode,
    plan_path: &mut Option<PathBuf>,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    run_turn_chain(
        ctx,
        id,
        text,
        std::collections::VecDeque::new(),
        target,
        conversation,
        tracker,
        thread_id,
        mode,
        plan_path,
        ops,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_turn_chain(
    ctx: &Ctx<'_>,
    id: TaskId,
    text: String,
    seed: std::collections::VecDeque<(TaskId, String)>,
    target: &mut Option<ModelTarget>,
    conversation: &mut Conversation,
    tracker: &mut ContextTracker,
    thread_id: &mut Option<i64>,
    mode: &mut Mode,
    plan_path: &mut Option<PathBuf>,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    let mut next = Some((id, text, seed));
    let mut pending: Vec<Op> = Vec::new();
    while let Some((turn_id, turn_text, turn_seed)) = next.take() {
        let (flow, deferred) = run_one_turn(
            ctx,
            turn_id,
            turn_text,
            turn_seed,
            target,
            conversation,
            tracker,
            thread_id,
            mode,
            plan_path,
            ops,
        )
        .await;
        pending.extend(deferred);
        match flow {
            TurnFlow::Shutdown => return Flow::Shutdown,
            TurnFlow::Idle => {}
            TurnFlow::Done(mut leftover) => {
                if let Some((next_id, next_text)) = leftover.pop_front() {
                    next = Some((next_id, next_text, leftover));
                }
            }
        }
    }
    drain_deferred(
        ctx,
        pending,
        target,
        conversation,
        tracker,
        thread_id,
        mode,
        plan_path,
        ops,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn drain_deferred(
    ctx: &Ctx<'_>,
    deferred: Vec<Op>,
    target: &mut Option<ModelTarget>,
    conversation: &mut Conversation,
    tracker: &mut ContextTracker,
    thread_id: &mut Option<i64>,
    mode: &mut Mode,
    plan_path: &mut Option<PathBuf>,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    for op in deferred {
        match op {
            Op::Compact { id, instructions } => {
                if let Flow::Shutdown = Box::pin(handle_compact(
                    ctx,
                    id,
                    instructions,
                    target,
                    conversation,
                    tracker,
                    thread_id,
                    mode,
                    plan_path,
                    ops,
                ))
                .await
                {
                    return Flow::Shutdown;
                }
            }
            Op::SubmitShell { id, command } => {
                if let Flow::Shutdown = Box::pin(handle_shell(
                    ctx,
                    id,
                    &command,
                    target,
                    conversation,
                    tracker,
                    thread_id,
                    mode,
                    plan_path,
                    ops,
                ))
                .await
                {
                    return Flow::Shutdown;
                }
            }
            Op::SetMode { mode: requested } => {
                apply_set_mode(ctx, requested, *thread_id, mode, plan_path).await;
            }
            other => handle_idle_op(other, ctx.store, *thread_id, target, ctx.events).await,
        }
    }
    Flow::Continue
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_shell(
    ctx: &Ctx<'_>,
    id: TaskId,
    command: &str,
    target: &mut Option<ModelTarget>,
    conversation: &mut Conversation,
    tracker: &mut ContextTracker,
    thread_id: &mut Option<i64>,
    mode: &mut Mode,
    plan_path: &mut Option<PathBuf>,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    if ctx.events.send(Event::TaskStarted { id }).await.is_err() {
        return Flow::Shutdown;
    }
    let stored_thread = match target.as_ref() {
        Some(resolved) => {
            ensure_thread(
                ctx.store,
                thread_id,
                resolved,
                thread_title(&format!("! {command}")),
            )
            .await
        }
        None => None,
    };
    let cwd = resolve_thread_cwd(ctx, stored_thread).await;
    let steering: crate::SteeringQueue = std::sync::Mutex::new(std::collections::VecDeque::new());
    let mut deferred: Vec<Op> = Vec::new();
    let outcome = {
        let work = run_shell_command(ctx.tools, command, &cwd);
        tokio::pin!(work);
        loop {
            tokio::select! {
                biased;
                output = &mut work => break ShellEnd::Done(output),
                maybe_op = ops.recv() => match maybe_op {
                    Some(Op::SubmitMessage { id: msg_id, text: msg_text }) => {
                        steering
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push_back((msg_id, msg_text));
                    }
                    Some(Op::DequeueMessage { id: msg_id }) => {
                        let removed = {
                            let mut queue = steering
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            queue
                                .iter()
                                .rposition(|(queued_id, _)| *queued_id == msg_id)
                                .and_then(|index| queue.remove(index))
                        };
                        if let Some((queued_id, queued_text)) = removed {
                            let _ = ctx
                                .events
                                .send(Event::MessageDequeued {
                                    id: queued_id,
                                    text: queued_text,
                                })
                                .await;
                        }
                    }
                    Some(Op::Interrupt { id: target_id }) if target_id == id => {
                        break ShellEnd::Interrupted;
                    }
                    Some(Op::Shutdown) | None => break ShellEnd::Shutdown,
                    Some(op) => deferred.push(op),
                },
            }
        }
    };

    let output = match outcome {
        ShellEnd::Shutdown => return Flow::Shutdown,
        ShellEnd::Interrupted => SHELL_INTERRUPTED.to_owned(),
        ShellEnd::Done(output) => output,
    };

    let encoded = shell::encode(command, &output);
    if conversation.is_empty() {
        conversation.push(
            Message::text(
                MessageRole::System,
                build_system_prompt(ctx.skills, ctx.instructions),
            ),
            None,
        );
    }
    let db_id = match stored_thread {
        Some(tid) => persist_shell_message(ctx, tid, &encoded).await,
        None => None,
    };
    conversation.push(Message::text(MessageRole::User, encoded), db_id);

    let _ = ctx.events.send(Event::ShellDone { id, output }).await;
    let _ = ctx
        .events
        .send(Event::TaskDone {
            id,
            interrupted: false,
        })
        .await;

    if let Flow::Shutdown = drain_deferred(
        ctx,
        deferred,
        target,
        conversation,
        tracker,
        thread_id,
        mode,
        plan_path,
        ops,
    )
    .await
    {
        return Flow::Shutdown;
    }
    let mut captured = std::mem::take(
        &mut *steering
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    drop(steering);
    if let Some((next_id, next_text)) = captured.pop_front() {
        return Box::pin(run_turn_chain(
            ctx,
            next_id,
            next_text,
            captured,
            target,
            conversation,
            tracker,
            thread_id,
            mode,
            plan_path,
            ops,
        ))
        .await;
    }
    Flow::Continue
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_compact(
    ctx: &Ctx<'_>,
    id: TaskId,
    instructions: Option<String>,
    target: &mut Option<ModelTarget>,
    conversation: &mut Conversation,
    tracker: &mut ContextTracker,
    thread_id: &mut Option<i64>,
    mode: &mut Mode,
    plan_path: &mut Option<PathBuf>,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    if conversation.is_empty() {
        let _ = ctx
            .events
            .send(Event::Notify {
                kind: goat_protocol::NotifyKind::Info,
                message: "nothing to compact".to_owned(),
            })
            .await;
        return Flow::Continue;
    }
    let Some(resolved) = target.clone() else {
        let _ = ctx
            .events
            .send(Event::Notify {
                kind: goat_protocol::NotifyKind::Error,
                message: "no model selected · /config to connect a provider".to_owned(),
            })
            .await;
        return Flow::Continue;
    };
    let resolved_provider = provider_for(
        ctx,
        &resolved.account,
        &goat_provider::ProviderId::from(resolved.provider.as_str()),
    );
    let Some(provider) = resolved_provider else {
        let _ = ctx
            .events
            .send(Event::Notify {
                kind: goat_protocol::NotifyKind::Error,
                message: format!("unknown provider: {}", resolved.provider),
            })
            .await;
        return Flow::Continue;
    };
    if ctx.events.send(Event::TaskStarted { id }).await.is_err() {
        return Flow::Shutdown;
    }
    let cwd = resolve_thread_cwd(ctx, *thread_id).await;
    let (tool_defs, exec_policy) = top_regime(ctx, provider.as_ref(), *mode);
    let ids = crate::TurnIds {
        stored_thread: *thread_id,
        turn_db_id: None,
        user_message_db_id: None,
    };
    let steering: crate::SteeringQueue = std::sync::Mutex::new(std::collections::VecDeque::new());
    let run = Run::top(id, &ids, &steering);
    let env = crate::LoopEnv {
        provider: provider.as_ref(),
        target: &resolved,
        tool_defs: &tool_defs,
        cwd: &cwd,
        allow_delegate: true,
        mode: *mode,
        plan_path: plan_path.clone(),
        exec_policy,
        transition: None,
    };
    let token = CancellationToken::new();
    let mut shutdown = false;
    let mut deferred: Vec<Op> = Vec::new();

    let result = {
        let work = crate::compaction::compact(
            ctx,
            &run,
            &env,
            conversation,
            tracker,
            instructions.as_deref(),
            &token,
        );
        tokio::pin!(work);
        loop {
            tokio::select! {
                biased;
                outcome = &mut work => break outcome,
                maybe_op = ops.recv() => match maybe_op {
                    Some(Op::SubmitMessage { id: msg_id, text: msg_text }) => {
                        steering
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push_back((msg_id, msg_text));
                    }
                    Some(Op::DequeueMessage { id: msg_id }) => {
                        let removed = {
                            let mut queue = steering
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            queue
                                .iter()
                                .rposition(|(queued_id, _)| *queued_id == msg_id)
                                .and_then(|index| queue.remove(index))
                        };
                        if let Some((queued_id, queued_text)) = removed {
                            let _ = ctx
                                .events
                                .send(Event::MessageDequeued {
                                    id: queued_id,
                                    text: queued_text,
                                })
                                .await;
                        }
                    }
                    Some(Op::Interrupt { id: target_id }) if target_id == id => token.cancel(),
                    Some(Op::Shutdown) | None => {
                        shutdown = true;
                        token.cancel();
                    }
                    Some(op) => deferred.push(op),
                },
            }
        }
    };

    match result {
        Ok(_) => {
            let _ = ctx
                .events
                .send(Event::TaskDone {
                    id,
                    interrupted: false,
                })
                .await;
        }
        Err(crate::compaction::CompactionError::Cancelled) => {
            let _ = ctx
                .events
                .send(Event::TaskDone {
                    id,
                    interrupted: true,
                })
                .await;
        }
        Err(crate::compaction::CompactionError::Failed(message)) => {
            emit_task_error(ctx, id, format!("compaction failed: {message}")).await;
        }
    }
    if shutdown {
        return Flow::Shutdown;
    }
    if let Flow::Shutdown = drain_deferred(
        ctx,
        deferred,
        target,
        conversation,
        tracker,
        thread_id,
        mode,
        plan_path,
        ops,
    )
    .await
    {
        return Flow::Shutdown;
    }
    let mut captured = std::mem::take(
        &mut *steering
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    drop(steering);
    if let Some((next_id, next_text)) = captured.pop_front() {
        return Box::pin(run_turn_chain(
            ctx,
            next_id,
            next_text,
            captured,
            target,
            conversation,
            tracker,
            thread_id,
            mode,
            plan_path,
            ops,
        ))
        .await;
    }
    Flow::Continue
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_one_turn(
    ctx: &Ctx<'_>,
    id: TaskId,
    text: String,
    seed: std::collections::VecDeque<(TaskId, String)>,
    target: &mut Option<ModelTarget>,
    conversation: &mut Conversation,
    tracker: &mut ContextTracker,
    thread_id: &mut Option<i64>,
    mode: &mut Mode,
    plan_path: &mut Option<PathBuf>,
    ops: &mut mpsc::Receiver<Op>,
) -> (TurnFlow, Vec<Op>) {
    let Some(resolved) = target.clone() else {
        emit_task_error(
            ctx,
            id,
            "no model selected · /config to connect a provider".to_owned(),
        )
        .await;
        return (TurnFlow::Idle, Vec::new());
    };
    let resolved_provider = provider_for(
        ctx,
        &resolved.account,
        &goat_provider::ProviderId::from(resolved.provider.as_str()),
    );
    let Some(provider) = resolved_provider else {
        emit_task_error(ctx, id, format!("unknown provider: {}", resolved.provider)).await;
        return (TurnFlow::Idle, Vec::new());
    };

    let ids = init_db_turn(ctx, id, &text, &resolved, thread_id).await;
    if mode.is_plan() && plan_path.is_none() {
        *plan_path = plan::resolve_plan_path(ids.stored_thread, &text);
        if let Some(tid) = ids.stored_thread
            && let Err(err) = ctx
                .store
                .update_thread_mode(tid, crate::mode_string(Mode::Plan), now_ms())
                .await
        {
            tracing::warn!(%err, "failed to persist thread mode");
        }
        if let Some(path) = plan_path.as_ref() {
            let _ = ctx
                .events
                .send(Event::ModeChanged {
                    mode: Mode::Plan,
                    plan_path: Some(path.display().to_string()),
                })
                .await;
        }
    }
    let system = {
        let mut text = build_system_prompt(ctx.skills, ctx.instructions);
        if mode.is_plan()
            && let Some(path) = plan_path.as_ref()
        {
            text.push_str(&plan::plan_segment(
                &path.display().to_string(),
                ctx.plan_shell,
            ));
        }
        text
    };
    if conversation.is_empty() {
        conversation.push(Message::text(MessageRole::System, system), None);
    } else if conversation.set_system(system) {
        tracker.invalidate();
    }
    conversation.push(
        Message::text(MessageRole::User, text.clone()),
        ids.user_message_db_id,
    );
    if ctx.events.send(Event::TaskStarted { id }).await.is_err() {
        finalize_turn(ctx, id, &TurnEnd::Shutdown, &ids).await;
        return (TurnFlow::Shutdown, Vec::new());
    }

    let cwd = resolve_thread_cwd(ctx, ids.stored_thread).await;
    let (tool_defs, exec_policy) = top_regime(ctx, provider.as_ref(), *mode);
    let transition_cell: plan::TransitionCell = std::sync::Mutex::new(None);
    let steering: crate::SteeringQueue = std::sync::Mutex::new(seed);
    let run = Run::top(id, &ids, &steering);
    let env = crate::LoopEnv {
        provider: provider.as_ref(),
        target: &resolved,
        tool_defs: &tool_defs,
        cwd: &cwd,
        allow_delegate: true,
        mode: *mode,
        plan_path: plan_path.clone(),
        exec_policy,
        transition: Some(&transition_cell),
    };
    let token = CancellationToken::new();
    let mut shutdown = false;
    let mut deferred: Vec<Op> = Vec::new();

    let outcome = {
        let core = core_loop(ctx, &run, &env, &token, conversation, tracker);
        tokio::pin!(core);
        loop {
            tokio::select! {
                biased;
                result = &mut core => break result,
                maybe_op = ops.recv() => match maybe_op {
                    Some(Op::Answer { call, answers, .. }) => {
                        if let Some(tx) = ctx.asks.lock().await.remove(&call) {
                            let _ = tx.send(answers);
                        }
                    }
                    Some(Op::ResolvePlan { call, decision, .. }) => {
                        if let Some(tx) = ctx.plans.lock().await.remove(&call) {
                            let _ = tx.send(decision);
                        }
                    }
                    Some(Op::SubmitMessage { id: msg_id, text: msg_text }) => {
                        steering
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push_back((msg_id, msg_text));
                    }
                    Some(Op::DequeueMessage { id: msg_id }) => {
                        let removed = {
                            let mut queue = steering
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            queue
                                .iter()
                                .rposition(|(queued_id, _)| *queued_id == msg_id)
                                .and_then(|index| queue.remove(index))
                        };
                        if let Some((queued_id, queued_text)) = removed {
                            let _ = ctx
                                .events
                                .send(Event::MessageDequeued {
                                    id: queued_id,
                                    text: queued_text,
                                })
                                .await;
                        }
                    }
                    Some(Op::Interrupt { id: target_id }) if target_id == id => token.cancel(),
                    Some(Op::Shutdown) | None => {
                        shutdown = true;
                        token.cancel();
                    }
                    Some(op) => deferred.push(op),
                },
            }
        }
    };

    let turn_end = match outcome {
        LoopOutcome::Completed | LoopOutcome::Transitioned => TurnEnd::Done,
        LoopOutcome::Failed(message) => TurnEnd::Failed(message),
        LoopOutcome::Cancelled => {
            if shutdown {
                TurnEnd::Shutdown
            } else {
                TurnEnd::Interrupted
            }
        }
    };
    finalize_turn(ctx, id, &turn_end, &ids).await;
    if matches!(turn_end, TurnEnd::Shutdown) {
        return (TurnFlow::Shutdown, deferred);
    }

    if matches!(turn_end, TurnEnd::Done) {
        let pending_transition = transition_cell
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let mut leftover = std::mem::take(
            &mut *steering
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        if let Some(transition) = pending_transition {
            *mode = transition.mode;
            if let Some(tid) = ids.stored_thread
                && let Err(err) = ctx
                    .store
                    .update_thread_mode(tid, crate::mode_string(transition.mode), now_ms())
                    .await
            {
                tracing::warn!(%err, "failed to persist thread mode");
            }
            let _ = ctx
                .events
                .send(Event::ModeChanged {
                    mode: transition.mode,
                    plan_path: plan_path.as_ref().map(|path| path.display().to_string()),
                })
                .await;
            let engine_id = TaskId(
                ctx.engine_ids
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            );
            let mut queue = std::collections::VecDeque::new();
            queue.push_back((engine_id, transition.inject));
            queue.append(&mut leftover);
            return (TurnFlow::Done(queue), deferred);
        }
        if !leftover.is_empty() {
            return (TurnFlow::Done(leftover), deferred);
        }
    }
    (TurnFlow::Idle, deferred)
}
