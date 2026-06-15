use goat_protocol::{Event as EngineEvent, NotifyKind, Op, TaskId, TranscriptEntry};

use super::{App, Overlay, ResumeIntent};
use crate::{ask::AskPicker, picker::ThreadPicker};

impl App {
    #[allow(clippy::too_many_lines)]
    pub(crate) fn on_engine(&mut self, event: EngineEvent) -> Vec<Op> {
        let mut ops = Vec::new();
        match event {
            EngineEvent::TaskStarted { id } => {
                if let Some(pos) = self
                    .queued
                    .iter()
                    .position(|(queued_id, _)| *queued_id == id)
                {
                    let (_, text) = self.queued.remove(pos);
                    self.reset_agents();
                    self.transcript.push_user(text);
                    self.follow = true;
                }
                self.active = Some(id);
                self.task_start = Some(std::time::Instant::now());
                self.thinking = false;
                self.turn_tokens = 0;
            }
            EngineEvent::ModelListChanged { entries } => {
                if let Overlay::Model(picker) = &mut self.overlay {
                    picker.set_entries(entries.clone());
                }
                self.models = entries;
                self.models_loaded = true;
            }
            EngineEvent::ModelSelected { target } => self.model = Some(target),
            EngineEvent::ThreadsListed { threads } => match self.pending_resume.take() {
                Some(ResumeIntent::Picker) => {
                    self.overlay = Overlay::Thread(ThreadPicker::new(threads));
                }
                Some(ResumeIntent::Index(index)) => match threads.get(index) {
                    Some(thread) => ops.push(Op::Resume {
                        thread_id: thread.id,
                    }),
                    None => {
                        self.push_toast(
                            NotifyKind::Error,
                            format!("no conversation #{}", index + 1),
                        );
                    }
                },
                None => {}
            },
            EngineEvent::ConversationRestored {
                target,
                entries,
                context_tokens,
                compaction_threshold,
                mode,
            } => {
                self.on_mode_changed(mode, None);
                self.transcript.clear();
                self.reset_agents();
                self.scroll = 0;
                self.follow = true;
                for entry in entries {
                    match entry {
                        TranscriptEntry::User(text) => self.transcript.push_user(text),
                        TranscriptEntry::Assistant(text) => {
                            self.transcript
                                .commit_text(&text, &self.highlighter, self.theme);
                        }
                        TranscriptEntry::Tool { call, outcome } => {
                            let id = call.id;
                            self.transcript.push_tool(call);
                            self.transcript
                                .finish_tool(id, outcome, self.picker.as_ref());
                        }
                        TranscriptEntry::Compaction {
                            tokens_before,
                            tokens_after,
                        } => {
                            self.transcript.push_compaction(tokens_before, tokens_after);
                        }
                        TranscriptEntry::Shell { command, output } => {
                            let id = TaskId(0);
                            self.transcript.push_shell(id, command);
                            self.transcript.finish_shell(id, output);
                        }
                    }
                }
                self.clear_ctx_indicator();
                self.compaction_threshold = compaction_threshold;
                if let Some(tokens) = context_tokens {
                    let key = (target.provider.clone(), target.account.clone());
                    self.usage_last.insert(
                        key,
                        goat_protocol::Usage {
                            input_tokens: tokens,
                            ..goat_protocol::Usage::default()
                        },
                    );
                }
                self.model = Some(target);
            }
            EngineEvent::ThinkingDelta { .. } => {
                self.thinking = true;
            }
            EngineEvent::LoginProviders { .. } => {}
            EngineEvent::CompactionStarted { id } => {
                if self.agent_index(id).is_none() {
                    self.compacting = true;
                }
                self.dirty = true;
            }
            EngineEvent::CompactionDone {
                id,
                ok,
                tokens_before,
                tokens_after,
                usage,
            } => {
                if let Some(i) = self.agent_index(id) {
                    if ok {
                        self.agent_runs[i]
                            .transcript
                            .push_compaction(tokens_before, tokens_after);
                    }
                } else {
                    self.compacting = false;
                    if ok {
                        self.turn_tokens +=
                            u64::from(usage.input_tokens) + u64::from(usage.output_tokens);
                        self.transcript.push_compaction(tokens_before, tokens_after);
                        if let Some(model) = &self.model {
                            let key = (model.provider.clone(), model.account.clone());
                            let total = self.usage_total.entry(key.clone()).or_default();
                            total.0 += u64::from(usage.input_tokens);
                            total.1 += u64::from(usage.output_tokens);
                            self.usage_last.insert(
                                key,
                                goat_protocol::Usage {
                                    input_tokens: tokens_after,
                                    ..goat_protocol::Usage::default()
                                },
                            );
                        }
                    }
                }
                self.dirty = true;
            }
            EngineEvent::UserMessage { id, text } => {
                if let Some(pos) = self
                    .queued
                    .iter()
                    .position(|(queued_id, _)| *queued_id == id)
                {
                    self.queued.remove(pos);
                }
                self.transcript.push_user(text);
                self.dirty = true;
            }
            EngineEvent::MessageDequeued { id, text } => {
                if let Some(pos) = self
                    .queued
                    .iter()
                    .position(|(queued_id, _)| *queued_id == id)
                {
                    self.queued.remove(pos);
                }
                let draft = self.composer.text();
                self.composer.clear();
                self.composer.insert_str(&text);
                if !draft.trim().is_empty() {
                    self.composer.insert_str("\n");
                    self.composer.insert_str(&draft);
                }
                self.dirty = true;
            }
            EngineEvent::Retrying {
                id,
                attempt,
                max_attempts,
                delay_ms,
                reason,
            } => {
                self.thinking = false;
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i].transcript.discard_stream();
                } else {
                    self.transcript.discard_stream();
                    self.retry = Some(super::RetryState {
                        attempt,
                        max_attempts,
                        reason,
                        until: std::time::Instant::now()
                            + std::time::Duration::from_millis(delay_ms),
                    });
                }
                self.dirty = true;
            }
            EngineEvent::AccountsChanged { providers } => {
                if let Overlay::Config(config) = &mut self.overlay {
                    config.set_providers(providers.clone());
                }
                self.account_entries = providers;
            }
            EngineEvent::SkillsChanged { skills } => {
                self.commands.set_skills(&skills);
            }
            EngineEvent::LoginStatus {
                message, done, ok, ..
            } => {
                if let Overlay::Config(config) = &mut self.overlay {
                    match (done, ok) {
                        (false, _) => config.set_account_status(message),
                        (true, true) => config.cancel_stage(),
                        (true, false) => config.set_error(message),
                    }
                }
            }
            EngineEvent::TextDelta { id, chunk } => {
                self.thinking = false;
                if self.agent_index(id).is_none() {
                    self.retry = None;
                }
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i].transcript.push_delta(&chunk);
                } else {
                    self.transcript.push_delta(&chunk);
                }
            }
            EngineEvent::TextDone { id, text } => {
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i]
                        .transcript
                        .commit_text(&text, &self.highlighter, self.theme);
                } else {
                    self.transcript
                        .commit_text(&text, &self.highlighter, self.theme);
                }
            }
            EngineEvent::ToolStarted { id, call } => {
                self.thinking = false;
                if self.agent_index(id).is_none() {
                    self.retry = None;
                }
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i].transcript.push_tool(call);
                } else {
                    self.transcript.push_tool(call);
                }
            }
            EngineEvent::ToolDone { id, call, outcome } => {
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i]
                        .transcript
                        .finish_tool(call, outcome, self.picker.as_ref());
                } else {
                    self.transcript
                        .finish_tool(call, outcome, self.picker.as_ref());
                }
            }
            EngineEvent::ShellDone { id, output } => {
                self.transcript.finish_shell(id, output);
            }
            EngineEvent::AgentStarted {
                id,
                agent_type,
                label,
                ..
            } => {
                self.agent_runs.push(super::AgentRunView {
                    id,
                    agent_type,
                    label,
                    transcript: crate::transcript::Transcript::default(),
                    done: None,
                });
            }
            EngineEvent::AgentDone { id, ok } => {
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i].done = Some(ok);
                    self.agent_runs[i]
                        .transcript
                        .complete(!ok, &self.highlighter, self.theme);
                }
            }
            EngineEvent::TaskDone { interrupted, .. } => {
                if !self.focused {
                    self.bell_pending = true;
                }
                self.transcript
                    .complete(interrupted, &self.highlighter, self.theme);
                self.active = None;
                self.active_shell = false;
                self.task_start = None;
                self.thinking = false;
                self.retry = None;
                self.compacting = false;
                if interrupted {
                    self.restore_queued_to_composer();
                }
            }
            EngineEvent::Error { message, .. } => {
                self.transcript
                    .push_error(message, &self.highlighter, self.theme);
                self.active = None;
                self.active_shell = false;
                self.task_start = None;
                self.thinking = false;
                self.retry = None;
            }
            EngineEvent::Notify { kind, message } => {
                self.toasts.push(crate::toast::Toast::new(kind, message));
                self.dirty = true;
            }
            EngineEvent::AskStarted {
                call, questions, ..
            } => {
                if !self.focused {
                    self.bell_pending = true;
                }
                self.overlay = Overlay::Ask(AskPicker::new(questions), call);
                self.dirty = true;
            }
            EngineEvent::AskDismissed { .. } => {
                if matches!(self.overlay, Overlay::Ask(..)) {
                    self.overlay = Overlay::None;
                    self.dirty = true;
                }
            }
            EngineEvent::Usage {
                id: _,
                usage,
                context_window,
                compaction_threshold,
            } => {
                self.turn_tokens += u64::from(usage.input_tokens) + u64::from(usage.output_tokens);
                if let Some(model) = &self.model {
                    let key = (model.provider.clone(), model.account.clone());
                    let total = self.usage_total.entry(key.clone()).or_default();
                    total.0 += u64::from(usage.input_tokens);
                    total.1 += u64::from(usage.output_tokens);
                    self.usage_last.insert(key, usage);
                }
                if let Some(w) = context_window {
                    self.context_window = Some(w);
                }
                if compaction_threshold.is_some() {
                    self.compaction_threshold = compaction_threshold;
                }
                self.dirty = true;
            }
            EngineEvent::RateLimits {
                provider,
                account,
                snapshot,
                cached_at,
            } => {
                self.rate_limits
                    .insert((provider, account), (snapshot, cached_at));
                self.dirty = true;
            }
            EngineEvent::ModeChanged { mode, plan_path } => {
                self.on_mode_changed(mode, plan_path);
            }
            EngineEvent::PlanProposed {
                id,
                call,
                plan,
                path,
            } => {
                self.on_plan_proposed(id, call, plan, path);
            }
            EngineEvent::PlanDismissed { .. } => {
                self.on_plan_dismissed();
            }
        }
        ops
    }

    pub(crate) fn agent_index(&self, id: TaskId) -> Option<usize> {
        self.agent_runs.iter().position(|run| run.id == id)
    }
}
