use goat_protocol::{Event as EngineEvent, Op, TaskId, TranscriptEntry};

use super::{App, Overlay, ResumeIntent};
use crate::picker::{AskPicker, ThreadPicker};

impl App {
    #[allow(clippy::too_many_lines)]
    pub(crate) fn on_engine(&mut self, event: EngineEvent) -> Vec<Op> {
        let mut ops = Vec::new();
        match event {
            EngineEvent::TaskStarted { .. } => {
                self.task_start = Some(std::time::Instant::now());
            }
            EngineEvent::ModelListChanged { entries } => {
                if let Overlay::Model(picker) = &mut self.overlay {
                    picker.set_entries(entries.clone());
                }
                self.models = entries;
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
                    None => self
                        .transcript
                        .push_error(format!("no conversation #{}", index + 1)),
                },
                None => {}
            },
            EngineEvent::ConversationRestored { target, entries } => {
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
                            self.transcript.finish_tool(id, outcome);
                        }
                    }
                }
                self.model = Some(target);
            }
            EngineEvent::ThinkingDelta { .. } | EngineEvent::LoginProviders { .. } => {}
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
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i].transcript.push_tool(call);
                } else {
                    self.transcript.push_tool(call);
                }
            }
            EngineEvent::ToolDone { id, call, outcome } => {
                if let Some(i) = self.agent_index(id) {
                    self.agent_runs[i].transcript.finish_tool(call, outcome);
                } else {
                    self.transcript.finish_tool(call, outcome);
                }
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
                self.transcript
                    .complete(interrupted, &self.highlighter, self.theme);
                self.active = None;
                self.task_start = None;
            }
            EngineEvent::Error { message, .. } => {
                self.transcript.push_error(message);
                self.active = None;
                self.task_start = None;
            }
            EngineEvent::Notify { kind, message } => {
                self.toasts.push(crate::toast::Toast::new(kind, message));
                self.dirty = true;
            }
            EngineEvent::AskStarted {
                call, questions, ..
            } => {
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
            } => {
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
        }
        if self.follow {
            self.scroll = u16::MAX;
        }
        ops
    }

    pub(crate) fn agent_index(&self, id: TaskId) -> Option<usize> {
        self.agent_runs.iter().position(|run| run.id == id)
    }
}
