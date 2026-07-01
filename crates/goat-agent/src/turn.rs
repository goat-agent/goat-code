use goat_protocol::{Event, InputAttachment, ModelTarget, Op, TaskId};
use goat_provider::{ContentBlock, Message, MessageRole, Provider, ToolDefinition};
use goat_store::Store;
use goat_tool::{SandboxPolicy, ToolContext, ToolError};
use goat_tools::ToolRegistry;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    Ctx, Flow, Run, SessionState,
    accounts::provider_for,
    persist::{
        effort_string, ensure_thread, finalize_turn, init_db_turn, now_ms, persist_shell_message,
        thread_title,
    },
    prompt::build_system_prompt,
    rounds::{LoopOutcome, core_loop},
    shell,
    threads::resolve_thread_cwd,
    tools_exec::build_tool_defs,
};

pub(crate) fn user_message(text: &str, attachments: &[InputAttachment]) -> Message {
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(ContentBlock::Text {
            text: text.to_owned(),
        });
    }
    for attachment in attachments {
        content.push(ContentBlock::Image {
            media_type: attachment.media_type.clone(),
            data: attachment.data.clone(),
        });
    }
    Message {
        role: MessageRole::User,
        content,
    }
}

fn top_regime(ctx: &Ctx<'_>, provider: &dyn Provider) -> (Vec<ToolDefinition>, SandboxPolicy) {
    (
        build_tool_defs(ctx, provider, None, true),
        SandboxPolicy::Full,
    )
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
    cwd: &std::path::Path,
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
        Op::ListThreads {} => {
            crate::threads::handle_list_threads(store, cwd, events).await;
        }
        _ => {}
    }
}

enum TurnFlow {
    Idle,
    Done(std::collections::VecDeque<crate::UserInput>),
    Shutdown,
}

pub(crate) async fn handle_turn(
    ctx: &Ctx<'_>,
    id: TaskId,
    text: String,
    display: Option<String>,
    attachments: Vec<InputAttachment>,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    run_turn_chain(
        ctx,
        crate::UserInput {
            id,
            text,
            display,
            attachments,
        },
        std::collections::VecDeque::new(),
        state,
        ops,
    )
    .await
}

async fn run_turn_chain(
    ctx: &Ctx<'_>,
    input: crate::UserInput,
    seed: std::collections::VecDeque<crate::UserInput>,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    let mut next = Some((input, seed));
    let mut pending: Vec<Op> = Vec::new();
    while let Some((turn_input, turn_seed)) = next.take() {
        let (flow, deferred) = run_one_turn(ctx, turn_input, turn_seed, state, ops).await;
        pending.extend(deferred);
        match flow {
            TurnFlow::Shutdown => return Flow::Shutdown,
            TurnFlow::Idle => {}
            TurnFlow::Done(mut leftover) => {
                if let Some(next_input) = leftover.pop_front() {
                    next = Some((next_input, leftover));
                }
            }
        }
    }
    drain_deferred(ctx, pending, state, ops).await
}

async fn drain_deferred(
    ctx: &Ctx<'_>,
    deferred: Vec<Op>,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    for op in deferred {
        match op {
            Op::Compact { id, instructions } => {
                if let Flow::Shutdown =
                    Box::pin(handle_compact(ctx, id, instructions, state, ops)).await
                {
                    return Flow::Shutdown;
                }
            }
            Op::SubmitShell { id, command } => {
                if let Flow::Shutdown = Box::pin(handle_shell(ctx, id, &command, state, ops)).await
                {
                    return Flow::Shutdown;
                }
            }
            other => {
                handle_idle_op(
                    other,
                    ctx.store,
                    ctx.cwd,
                    state.thread_id,
                    &mut state.target,
                    ctx.events,
                )
                .await;
            }
        }
    }
    Flow::Continue
}

pub(crate) async fn handle_shell(
    ctx: &Ctx<'_>,
    id: TaskId,
    command: &str,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    if ctx.events.send(Event::TaskStarted { id }).await.is_err() {
        return Flow::Shutdown;
    }
    let stored_thread = match state.target.as_ref() {
        Some(resolved) => {
            ensure_thread(
                ctx.store,
                ctx.cwd,
                &mut state.thread_id,
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
                    Some(Op::SubmitMessage { id: msg_id, text: msg_text, display, attachments }) => {
                        steering
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push_back(crate::UserInput { id: msg_id, text: msg_text, display, attachments });
                    }
                    Some(Op::DequeueMessage { id: msg_id }) => {
                        let removed = {
                            let mut queue = steering
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            queue
                                .iter()
                                .rposition(|queued| queued.id == msg_id)
                                .and_then(|index| queue.remove(index))
                        };
                        if let Some(queued) = removed {
                            let _ = ctx
                                .events
                                .send(Event::MessageDequeued {
                                    id: queued.id,
                                    text: queued.text,
                                    display: queued.display,
                                    attachments: queued.attachments,
                                })
                                .await;
                        }
                    }
                    Some(Op::Interrupt { id: target_id }) if target_id == id => {
                        break ShellEnd::Interrupted;
                    }
                    Some(Op::Shutdown {}) | None => break ShellEnd::Shutdown,
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
    if state.conversation.is_empty() {
        state.conversation.push(
            Message::text(
                MessageRole::System,
                build_system_prompt(ctx.cwd, ctx.skills, ctx.instructions, ctx.date),
            ),
            None,
        );
    }
    let db_id = match stored_thread {
        Some(tid) => persist_shell_message(ctx, tid, &encoded).await,
        None => None,
    };
    state
        .conversation
        .push(Message::text(MessageRole::User, encoded), db_id);

    let _ = ctx.events.send(Event::ShellDone { id, output }).await;
    let _ = ctx
        .events
        .send(Event::TaskDone {
            id,
            interrupted: false,
        })
        .await;

    if let Flow::Shutdown = drain_deferred(ctx, deferred, state, ops).await {
        return Flow::Shutdown;
    }
    let mut captured = std::mem::take(
        &mut *steering
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    drop(steering);
    if let Some(next_input) = captured.pop_front() {
        return Box::pin(run_turn_chain(ctx, next_input, captured, state, ops)).await;
    }
    Flow::Continue
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_compact(
    ctx: &Ctx<'_>,
    id: TaskId,
    instructions: Option<String>,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
) -> Flow {
    if state.conversation.is_empty() {
        let _ = ctx
            .events
            .send(Event::Notify {
                kind: goat_protocol::NotifyKind::Info,
                message: "nothing to compact".to_owned(),
            })
            .await;
        return Flow::Continue;
    }
    let Some(resolved) = state.target.clone() else {
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
    let cwd = resolve_thread_cwd(ctx, state.thread_id).await;
    let (tool_defs, exec_policy) = top_regime(ctx, provider.as_ref());
    let ids = crate::TurnIds {
        stored_thread: state.thread_id,
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
        exec_policy,
    };
    let token = CancellationToken::new();
    let mut shutdown = false;
    let mut deferred: Vec<Op> = Vec::new();

    let result = {
        let work = crate::compaction::compact(
            ctx,
            &run,
            &env,
            &mut state.conversation,
            &mut state.tracker,
            instructions.as_deref(),
            &token,
        );
        tokio::pin!(work);
        loop {
            tokio::select! {
                biased;
                outcome = &mut work => break outcome,
                maybe_op = ops.recv() => match maybe_op {
                    Some(Op::SubmitMessage { id: msg_id, text: msg_text, display, attachments }) => {
                        steering
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push_back(crate::UserInput { id: msg_id, text: msg_text, display, attachments });
                    }
                    Some(Op::DequeueMessage { id: msg_id }) => {
                        let removed = {
                            let mut queue = steering
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            queue
                                .iter()
                                .rposition(|queued| queued.id == msg_id)
                                .and_then(|index| queue.remove(index))
                        };
                        if let Some(queued) = removed {
                            let _ = ctx
                                .events
                                .send(Event::MessageDequeued {
                                    id: queued.id,
                                    text: queued.text,
                                    display: queued.display,
                                    attachments: queued.attachments,
                                })
                                .await;
                        }
                    }
                    Some(Op::Interrupt { id: target_id }) if target_id == id => token.cancel(),
                    Some(Op::Shutdown {}) | None => {
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
    if let Flow::Shutdown = drain_deferred(ctx, deferred, state, ops).await {
        return Flow::Shutdown;
    }
    let mut captured = std::mem::take(
        &mut *steering
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    drop(steering);
    if let Some(next_input) = captured.pop_front() {
        return Box::pin(run_turn_chain(ctx, next_input, captured, state, ops)).await;
    }
    Flow::Continue
}

#[allow(clippy::too_many_lines)]
async fn run_one_turn(
    ctx: &Ctx<'_>,
    input: crate::UserInput,
    seed: std::collections::VecDeque<crate::UserInput>,
    state: &mut SessionState,
    ops: &mut mpsc::Receiver<Op>,
) -> (TurnFlow, Vec<Op>) {
    let id = input.id;
    let text = input.text;
    let attachments = input.attachments;
    let Some(resolved) = state.target.clone() else {
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

    let message = user_message(&text, &attachments);
    let ids = init_db_turn(
        ctx,
        id,
        &message,
        &text,
        &attachments,
        &resolved,
        &mut state.thread_id,
    )
    .await;
    let system = build_system_prompt(ctx.cwd, ctx.skills, ctx.instructions, ctx.date);
    if state.conversation.is_empty() {
        state
            .conversation
            .push(Message::text(MessageRole::System, system), None);
    } else if state.conversation.set_system(system) {
        state.tracker.invalidate();
    }
    state.conversation.push(message, ids.user_message_db_id);
    if ctx
        .events
        .send(Event::UserMessage {
            id,
            text: text.clone(),
            display: input.display.clone(),
            attachments: attachments.clone(),
        })
        .await
        .is_err()
    {
        finalize_turn(ctx, id, &TurnEnd::Shutdown, &ids).await;
        return (TurnFlow::Shutdown, Vec::new());
    }
    if ctx.events.send(Event::TaskStarted { id }).await.is_err() {
        finalize_turn(ctx, id, &TurnEnd::Shutdown, &ids).await;
        return (TurnFlow::Shutdown, Vec::new());
    }

    let cwd = resolve_thread_cwd(ctx, ids.stored_thread).await;
    let (tool_defs, exec_policy) = top_regime(ctx, provider.as_ref());
    let steering: crate::SteeringQueue = std::sync::Mutex::new(seed);
    let run = Run::top(id, &ids, &steering);
    let env = crate::LoopEnv {
        provider: provider.as_ref(),
        target: &resolved,
        tool_defs: &tool_defs,
        cwd: &cwd,
        allow_delegate: true,
        exec_policy,
    };
    let token = CancellationToken::new();
    let mut shutdown = false;
    let mut deferred: Vec<Op> = Vec::new();

    let outcome = {
        let core = core_loop(
            ctx,
            &run,
            &env,
            &token,
            &mut state.conversation,
            &mut state.tracker,
        );
        tokio::pin!(core);
        loop {
            tokio::select! {
                biased;
                result = &mut core => break result,
                maybe_op = ops.recv() => match maybe_op {
                    Some(Op::Answer { call, answers, .. }) => {
                        if let Some(tx) = ctx.asks.lock().await.remove(&call) {
                            let _ = tx.send(answers);
                            let _ = ctx
                                .events
                                .send(Event::AskDismissed { id, call })
                                .await;
                        }
                    }
                    Some(Op::SubmitMessage { id: msg_id, text: msg_text, display, attachments }) => {
                        steering
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push_back(crate::UserInput { id: msg_id, text: msg_text, display, attachments });
                    }
                    Some(Op::DequeueMessage { id: msg_id }) => {
                        let removed = {
                            let mut queue = steering
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            queue
                                .iter()
                                .rposition(|queued| queued.id == msg_id)
                                .and_then(|index| queue.remove(index))
                        };
                        if let Some(queued) = removed {
                            let _ = ctx
                                .events
                                .send(Event::MessageDequeued {
                                    id: queued.id,
                                    text: queued.text,
                                    display: queued.display,
                                    attachments: queued.attachments,
                                })
                                .await;
                        }
                    }
                    Some(Op::Interrupt { id: target_id }) if target_id == id => token.cancel(),
                    Some(Op::Shutdown {}) | None => {
                        shutdown = true;
                        token.cancel();
                    }
                    Some(op) => deferred.push(op),
                },
            }
        }
    };

    let turn_end = match outcome {
        LoopOutcome::Completed => TurnEnd::Done,
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
        let leftover = std::mem::take(
            &mut *steering
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        if !leftover.is_empty() {
            return (TurnFlow::Done(leftover), deferred);
        }
    }
    (TurnFlow::Idle, deferred)
}
