use goat_protocol::Event;
use goat_provider::{
    ContentBlock, Message, MessageRole, Provider, Request, StreamError, StreamEvent, ToolChoice,
};
use goat_tool::ToolContext;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    Ctx, LoopEnv, Run,
    compaction::ContextTracker,
    conversation::Conversation,
    persist::{now_ms, persist_message},
    rate_limit_cache,
    tools_exec::run_tool_batch,
};

pub(crate) const ROUNDS_BACKSTOP: usize = 1000;

pub(crate) enum RoundEnd {
    Completed,
    Cancelled,
    Failed(StreamError),
}

pub(crate) struct RoundResult {
    pub(crate) end: RoundEnd,
    pub(crate) raw: String,
    pub(crate) thinking: Option<(String, String)>,
    pub(crate) redacted: Vec<String>,
    pub(crate) pending_calls: Vec<(String, String, String)>,
    pub(crate) usage: Option<goat_provider::Usage>,
    pub(crate) rate_limits: Option<goat_provider::RateLimitSnapshot>,
}

impl RoundResult {
    pub(crate) fn ended(end: RoundEnd) -> Self {
        Self {
            end,
            raw: String::new(),
            thinking: None,
            redacted: Vec::new(),
            pending_calls: Vec::new(),
            usage: None,
            rate_limits: None,
        }
    }
}

pub(crate) enum RoundOutcome {
    Done,
    Continue,
    Cancelled,
}

pub(crate) enum LoopOutcome {
    Completed,
    Cancelled,
    Failed(String),
}

async fn drain_steering(ctx: &Ctx<'_>, run: &Run<'_>, conversation: &mut Conversation) {
    let Some(queue) = run.steering() else {
        return;
    };
    loop {
        let entry = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front();
        let Some((msg_id, text)) = entry else {
            break;
        };
        let message = Message::text(MessageRole::User, text.clone());
        let db_id = match run.ids() {
            Some(ids) => persist_message(ctx, ids, &message).await,
            None => None,
        };
        conversation.push(message, db_id);
        let _ = ctx
            .events
            .send(Event::UserMessage { id: msg_id, text })
            .await;
    }
}

pub(crate) async fn run_round(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    provider: &dyn Provider,
    request: Request,
    token: &CancellationToken,
) -> RoundResult {
    let (mev_tx, mut mev_rx) = mpsc::channel(64);
    let handle = provider.stream(request, mev_tx);
    let mut raw = String::new();
    let mut thinking = String::new();
    let mut signature = String::new();
    let mut redacted: Vec<String> = Vec::new();
    let mut pending_calls: Vec<(String, String, String)> = Vec::new();
    let mut usage: Option<goat_provider::Usage> = None;
    let mut rate_limits: Option<goat_provider::RateLimitSnapshot> = None;
    let end = loop {
        tokio::select! {
            biased;
            () = token.cancelled() => {
                handle.abort();
                break RoundEnd::Cancelled;
            }
            maybe_event = mev_rx.recv() => match maybe_event {
                Some(StreamEvent::TextDelta { text }) => {
                    raw.push_str(&text);
                    let _ = ctx
                        .events
                        .send(Event::TextDelta { id: run.id, chunk: text })
                        .await;
                }
                Some(StreamEvent::ThinkingDelta { text }) => {
                    thinking.push_str(&text);
                    let _ = ctx
                        .events
                        .send(Event::ThinkingDelta { id: run.id, chunk: text })
                        .await;
                }
                Some(StreamEvent::ThinkingSignature { signature: sig }) => {
                    signature.push_str(&sig);
                }
                Some(StreamEvent::RedactedThinking { data }) => {
                    redacted.push(data);
                }
                Some(StreamEvent::ToolCall { id: vendor_id, name, input }) => {
                    pending_calls.push((vendor_id, name, input));
                }
                Some(StreamEvent::Usage { usage: u }) => {
                    usage = Some(u);
                }
                Some(StreamEvent::RateLimits { snapshot }) => {
                    rate_limits = Some(snapshot);
                }
                Some(StreamEvent::Completed) | None => break RoundEnd::Completed,
                Some(StreamEvent::Failed { error }) => break RoundEnd::Failed(error),
            }
        }
    };
    let thinking = (!thinking.is_empty()).then_some((thinking, signature));
    RoundResult {
        end,
        raw,
        thinking,
        redacted,
        pending_calls,
        usage,
        rate_limits,
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn process_round_output(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    env: &LoopEnv<'_>,
    round: RoundResult,
    conversation: &mut Conversation,
    tracker: &mut ContextTracker,
    rounds: usize,
    call_seq: &mut u64,
    tool_ctx: &ToolContext,
    token: &CancellationToken,
) -> RoundOutcome {
    if let Some(usage) = round.usage.clone()
        && run.is_top()
    {
        let context_window = env.provider.context_window(&env.target.model);
        let compaction_threshold = context_window.map(crate::compaction::proactive_limit);
        let _ = ctx
            .events
            .send(Event::Usage {
                id: run.id,
                usage,
                context_window,
                compaction_threshold,
            })
            .await;
    }
    if let Some(snapshot) = round.rate_limits
        && run.is_top()
    {
        let now = now_ms() / 1000;
        let serialized = {
            let mut cache = ctx
                .rl_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache.upsert(
                &env.target.provider,
                &env.target.account,
                snapshot.windows.clone(),
                now,
            );
            cache.to_json()
        };
        if let (Some(path), Some(json)) = (ctx.rl_path, serialized) {
            let path = path.to_owned();
            tokio::task::spawn_blocking(move || rate_limit_cache::write(&path, &json));
        }
        let _ = ctx
            .events
            .send(Event::RateLimits {
                provider: env.target.provider.clone(),
                account: env.target.account.clone(),
                snapshot,
                cached_at: now,
            })
            .await;
    }
    let raw = round.raw;
    let pending_calls = round.pending_calls;
    let shown_text = (!raw.is_empty()).then(|| raw.clone());
    if !raw.is_empty()
        || !pending_calls.is_empty()
        || round.thinking.is_some()
        || !round.redacted.is_empty()
    {
        let mut content = Vec::new();
        if let Some((text, signature)) = &round.thinking {
            content.push(ContentBlock::Thinking {
                text: text.clone(),
                signature: signature.clone(),
            });
        }
        for data in &round.redacted {
            content.push(ContentBlock::RedactedThinking { data: data.clone() });
        }
        if !raw.is_empty() {
            content.push(ContentBlock::Text { text: raw.clone() });
        }
        for (vendor_id, name, input_json) in &pending_calls {
            let input_val = serde_json::from_str(input_json).unwrap_or(serde_json::Value::Null);
            content.push(ContentBlock::ToolUse {
                id: vendor_id.clone(),
                name: name.clone(),
                input: input_val,
            });
        }
        let message = Message {
            role: MessageRole::Assistant,
            content,
        };
        let db_id = match run.ids() {
            Some(ids) => persist_message(ctx, ids, &message).await,
            None => None,
        };
        conversation.push(message, db_id);
    }
    if let Some(usage) = &round.usage {
        tracker.record(conversation.len(), usage);
    }
    if let Some(shown) = shown_text {
        let _ = ctx
            .events
            .send(Event::TextDone {
                id: run.id,
                text: shown,
            })
            .await;
    }
    if pending_calls.is_empty() {
        if run.steering_pending() {
            return RoundOutcome::Continue;
        }
        return RoundOutcome::Done;
    }
    if rounds >= ROUNDS_BACKSTOP {
        tracing::warn!(rounds, "tool round backstop reached; ending run");
        let synthetic: Vec<ContentBlock> = pending_calls
            .iter()
            .map(|(vendor_id, _, _)| {
                ContentBlock::text_result(vendor_id.clone(), "tool round limit reached", true)
            })
            .collect();
        let message = Message {
            role: MessageRole::User,
            content: synthetic,
        };
        let db_id = match run.ids() {
            Some(ids) => persist_message(ctx, ids, &message).await,
            None => None,
        };
        conversation.push(message, db_id);
        return RoundOutcome::Done;
    }
    let batch = run_tool_batch(ctx, run, env, &pending_calls, call_seq, tool_ctx, token).await;
    let message = Message {
        role: MessageRole::User,
        content: batch.tool_results,
    };
    let db_id = match run.ids() {
        Some(ids) => persist_message(ctx, ids, &message).await,
        None => None,
    };
    conversation.push(message, db_id);
    if batch.cancelled {
        RoundOutcome::Cancelled
    } else {
        RoundOutcome::Continue
    }
}

pub(crate) async fn core_loop(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    env: &LoopEnv<'_>,
    token: &CancellationToken,
    conversation: &mut Conversation,
    tracker: &mut ContextTracker,
) -> LoopOutcome {
    let tool_ctx = match ToolContext::new(env.cwd) {
        Ok(tool_ctx) => tool_ctx,
        Err(err) => return LoopOutcome::Failed(err.to_string()),
    };
    let mut rounds = 0usize;
    let mut call_seq = 0u64;
    let mut compacted_for_overflow = false;
    loop {
        rounds += 1;
        drain_steering(ctx, run, conversation).await;
        if let Some(window) = env.provider.context_window(&env.target.model)
            && tracker.estimate(conversation.messages(), env.tool_defs)
                > crate::compaction::proactive_limit(window)
        {
            match crate::compaction::compact(ctx, run, env, conversation, tracker, None, token)
                .await
            {
                Ok(_) => {}
                Err(crate::compaction::CompactionError::Cancelled) => {
                    return LoopOutcome::Cancelled;
                }
                Err(crate::compaction::CompactionError::Failed(message)) => {
                    tracing::warn!(%message, "proactive compaction failed");
                }
            }
        }
        let request = Request {
            model: env.target.model.clone(),
            messages: conversation.messages().to_vec(),
            tools: env.tool_defs.to_vec(),
            effort: env.target.effort,
            tool_choice: ToolChoice::Auto,
        };
        let round = crate::retry::run_round_with_retry(ctx, run, env, &request, token).await;
        match &round.end {
            RoundEnd::Cancelled => return LoopOutcome::Cancelled,
            RoundEnd::Failed(StreamError::ContextOverflow { .. }) if !compacted_for_overflow => {
                match crate::compaction::compact(ctx, run, env, conversation, tracker, None, token)
                    .await
                {
                    Ok(_) => {
                        compacted_for_overflow = true;
                        continue;
                    }
                    Err(crate::compaction::CompactionError::Cancelled) => {
                        return LoopOutcome::Cancelled;
                    }
                    Err(crate::compaction::CompactionError::Failed(message)) => {
                        return LoopOutcome::Failed(message);
                    }
                }
            }
            RoundEnd::Failed(error) => {
                return LoopOutcome::Failed(crate::retry::failure_message(error, env.target));
            }
            RoundEnd::Completed => {}
        }
        match process_round_output(
            ctx,
            run,
            env,
            round,
            conversation,
            tracker,
            rounds,
            &mut call_seq,
            &tool_ctx,
            token,
        )
        .await
        {
            RoundOutcome::Done => return LoopOutcome::Completed,
            RoundOutcome::Cancelled => return LoopOutcome::Cancelled,
            RoundOutcome::Continue => {}
        }
        compacted_for_overflow = false;
    }
}
