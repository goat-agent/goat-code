use goat_protocol::{Event, ModelTarget, Op, TaskId};
use goat_provider::{Message, MessageRole};
use goat_store::Store;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    Ctx, Flow, Run,
    accounts::provider_for,
    compaction::ContextTracker,
    conversation::Conversation,
    persist::{effort_string, finalize_turn, init_db_turn, now_ms},
    prompt::build_system_prompt,
    rounds::{LoopOutcome, core_loop},
    threads::resolve_thread_cwd,
    tools_exec::build_tool_defs,
};

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
    for op in pending {
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
                    ops,
                ))
                .await
                {
                    return Flow::Shutdown;
                }
            }
            other => handle_idle_op(other, ctx.store, *thread_id, target, ctx.events).await,
        }
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
    let tool_defs = build_tool_defs(ctx, provider.as_ref(), None, true);
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
    for op in deferred {
        handle_idle_op(op, ctx.store, *thread_id, target, ctx.events).await;
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
            ops,
        ))
        .await;
    }
    Flow::Continue
}

#[allow(clippy::too_many_arguments)]
async fn run_one_turn(
    ctx: &Ctx<'_>,
    id: TaskId,
    text: String,
    seed: std::collections::VecDeque<(TaskId, String)>,
    target: &mut Option<ModelTarget>,
    conversation: &mut Conversation,
    tracker: &mut ContextTracker,
    thread_id: &mut Option<i64>,
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

    if conversation.is_empty() {
        conversation.push(
            Message::text(
                MessageRole::System,
                build_system_prompt(ctx.skills, ctx.instructions),
            ),
            None,
        );
    }
    let ids = init_db_turn(ctx, id, &text, &resolved, thread_id).await;
    conversation.push(
        Message::text(MessageRole::User, text.clone()),
        ids.user_message_db_id,
    );
    if ctx.events.send(Event::TaskStarted { id }).await.is_err() {
        finalize_turn(ctx, id, &TurnEnd::Shutdown, &ids).await;
        return (TurnFlow::Shutdown, Vec::new());
    }

    let cwd = resolve_thread_cwd(ctx, ids.stored_thread).await;
    let tool_defs = build_tool_defs(ctx, provider.as_ref(), None, true);
    let steering: crate::SteeringQueue = std::sync::Mutex::new(seed);
    let run = Run::top(id, &ids, &steering);
    let env = crate::LoopEnv {
        provider: provider.as_ref(),
        target: &resolved,
        tool_defs: &tool_defs,
        cwd: &cwd,
        allow_delegate: true,
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
