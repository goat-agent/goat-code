use goat_protocol::{
    Effort, Event, ModelTarget, NotifyKind, SkillInfo, ThreadSummary, ToolCall, ToolCallId,
    ToolOutcome, TranscriptEntry,
};
use goat_provider::{ContentBlock, Message, MessageRole};
use goat_store::Store;
use goat_tools::ToolRegistry;
use tokio::sync::mpsc;

use crate::{
    Ctx,
    compaction::ContextTracker,
    conversation::Conversation,
    prompt::build_system_prompt,
    tools_exec::{call_display, summarize_line},
};

pub(crate) fn parse_content_blocks(body: &str) -> Vec<ContentBlock> {
    serde_json::from_str::<Vec<ContentBlock>>(body).unwrap_or_else(|_| {
        vec![ContentBlock::Text {
            text: body.to_owned(),
        }]
    })
}

pub(crate) async fn resolve_thread_cwd(
    ctx: &Ctx<'_>,
    stored_thread: Option<i64>,
) -> std::path::PathBuf {
    match stored_thread {
        Some(tid) => ctx
            .store
            .get_thread(tid)
            .await
            .ok()
            .flatten()
            .map(|thread| thread.cwd)
            .filter(|cwd| !cwd.is_empty())
            .map_or_else(
                || std::env::current_dir().unwrap_or_default(),
                std::path::PathBuf::from,
            ),
        None => std::env::current_dir().unwrap_or_default(),
    }
}

pub(crate) async fn handle_list_threads(store: &Store, events: &mpsc::Sender<Event>) {
    let cwd = std::env::current_dir()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let threads = store.list_threads_in(cwd, 50).await.unwrap_or_default();
    let summaries = threads
        .into_iter()
        .map(|thread| ThreadSummary {
            model: format!("{}/{}", thread.provider, thread.model),
            title: thread
                .title
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| format!("{}/{}", thread.provider, thread.model)),
            id: thread.id,
            updated_at: thread.updated_at,
        })
        .collect();
    let _ = events
        .send(Event::ThreadsListed { threads: summaries })
        .await;
}

pub(crate) async fn handle_rename(
    store: &Store,
    thread_id: Option<i64>,
    title: String,
    events: &mpsc::Sender<Event>,
) {
    let Some(tid) = thread_id else {
        let _ = events
            .send(Event::Notify {
                kind: NotifyKind::Error,
                message: "no active conversation to rename".to_owned(),
            })
            .await;
        return;
    };
    match store.update_thread_title(tid, title.clone()).await {
        Ok(()) => {
            let _ = events
                .send(Event::Notify {
                    kind: NotifyKind::Success,
                    message: format!("renamed to \"{title}\""),
                })
                .await;
        }
        Err(err) => {
            tracing::warn!(%err, "failed to rename thread");
            let _ = events
                .send(Event::Notify {
                    kind: NotifyKind::Error,
                    message: "failed to rename conversation".to_owned(),
                })
                .await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_resume(
    store: &Store,
    skills: &[SkillInfo],
    tools: &ToolRegistry,
    instructions: Option<&str>,
    tid: i64,
    target: &mut Option<ModelTarget>,
    conversation: &mut Conversation,
    tracker: &mut ContextTracker,
    thread_id: &mut Option<i64>,
    events: &mpsc::Sender<Event>,
) {
    let Some(thread) = store.get_thread(tid).await.ok().flatten() else {
        return;
    };
    let new_target = ModelTarget {
        provider: thread.provider.clone(),
        model: thread.model.clone(),
        account: thread.account.clone(),
        effort: thread.effort.as_deref().and_then(Effort::parse),
    };
    let messages = store.get_messages(tid).await.unwrap_or_default();
    let compactions = store.compactions_for_thread(tid).await.unwrap_or_default();
    let mut parsed: Vec<(i64, MessageRole, Vec<ContentBlock>)> = Vec::new();
    let mut entries: Vec<TranscriptEntry> = Vec::new();
    let mut tool_uses: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    let mut tool_seq: u64 = 0;
    let mut next_compaction = 0usize;
    for stored in messages {
        while next_compaction < compactions.len()
            && compactions[next_compaction].after_message_id < stored.id
        {
            let compaction = &compactions[next_compaction];
            entries.push(TranscriptEntry::Compaction {
                tokens_before: u32::try_from(compaction.tokens_before).unwrap_or(0),
                tokens_after: u32::try_from(compaction.tokens_after).unwrap_or(0),
            });
            next_compaction += 1;
        }
        if stored.role == "shell" {
            let content = parse_content_blocks(&stored.body);
            if let Some(ContentBlock::Text { text }) = content.first() {
                match crate::shell::decode(text) {
                    Some((command, output)) => {
                        entries.push(TranscriptEntry::Shell { command, output });
                    }
                    None => entries.push(TranscriptEntry::User(text.clone())),
                }
            }
            parsed.push((stored.id, MessageRole::User, content));
            continue;
        }
        let role = match stored.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            _ => continue,
        };
        let content = parse_content_blocks(&stored.body);
        for block in &content {
            match block {
                ContentBlock::Text { text } => match role {
                    MessageRole::User => entries.push(TranscriptEntry::User(text.clone())),
                    MessageRole::Assistant => {
                        entries.push(TranscriptEntry::Assistant(text.clone()));
                    }
                    MessageRole::System => {}
                },
                ContentBlock::ToolUse { id, name, input } => {
                    tool_uses.insert(id.clone(), (name.clone(), input.to_string()));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    if let Some((name, input)) = tool_uses.remove(tool_use_id) {
                        tool_seq += 1;
                        let display = call_display(tools, &name, &input);
                        let summary = if *is_error {
                            summarize_line(&ContentBlock::tool_result_text(content))
                        } else {
                            None
                        };
                        entries.push(TranscriptEntry::Tool {
                            call: ToolCall {
                                id: ToolCallId(tool_seq),
                                name,
                                display,
                            },
                            outcome: ToolOutcome {
                                ok: !is_error,
                                summary,
                            },
                        });
                    }
                }
                ContentBlock::Image { .. }
                | ContentBlock::Thinking { .. }
                | ContentBlock::RedactedThinking { .. } => {}
            }
        }
        parsed.push((stored.id, role, content));
    }
    while next_compaction < compactions.len() {
        let compaction = &compactions[next_compaction];
        entries.push(TranscriptEntry::Compaction {
            tokens_before: u32::try_from(compaction.tokens_before).unwrap_or(0),
            tokens_after: u32::try_from(compaction.tokens_after).unwrap_or(0),
        });
        next_compaction += 1;
    }
    let mut new_history: Vec<(Message, Option<i64>)> = vec![(
        Message::text(
            MessageRole::System,
            build_system_prompt(skills, instructions),
        ),
        None,
    )];
    if let Some(latest) = compactions.last() {
        new_history.push((crate::compaction::summary_message(&latest.summary), None));
        for (id, role, content) in parsed {
            let include = latest.preserved_message_ids.contains(&id)
                || latest
                    .tail_from_message_id
                    .is_some_and(|tail| id >= tail && id <= latest.after_message_id)
                || id > latest.after_message_id;
            if include {
                new_history.push((Message { role, content }, Some(id)));
            }
        }
    } else {
        for (id, role, content) in parsed {
            new_history.push((Message { role, content }, Some(id)));
        }
    }
    conversation.replace(new_history);
    tracker.invalidate();
    let context_tokens = Some(tracker.estimate(conversation.messages(), &[]));
    *thread_id = Some(tid);
    *target = Some(new_target.clone());
    let _ = events
        .send(Event::ConversationRestored {
            target: new_target,
            entries,
            context_tokens,
            compaction_threshold: None,
        })
        .await;
}
