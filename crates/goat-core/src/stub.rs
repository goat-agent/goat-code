use std::time::Duration;

use goat_protocol::{Conversation, Event, Message, Op, Role, TaskId, ToolCall};
use tokio::{sync::mpsc, task::JoinHandle, time};

use crate::engine::Engine;

const DELTA_DELAY: Duration = Duration::from_millis(30);

const REPLIES: &[Reply] = &[
    Reply::Markdown(
        "Here's how to fix the bug:\n\n\
         ## Steps\n\n\
         1. Open `src/main.rs`\n\
         2. Replace the broken function\n\
         3. Run the tests\n\n\
         ```rust\nfn parse_args(input: &str) -> Result<Args> {\n    input.parse().map_err(Into::into)\n}\n```\n\n\
         The key change is using `map_err` instead of `unwrap`. \
         This makes the **error handling** explicit and avoids a panic at runtime.",
    ),
    Reply::WithTools {
        tools: &[
            ("read", Some("src/main.rs"), true),
            ("grep", Some("parse_args"), true),
            ("bash", Some("cargo test --workspace"), false),
        ],
        text: "I read the file and searched the codebase.\n\n\
               Running the tests **failed** — here's what I found:\n\n\
               ```\nthread 'main' panicked at 'assertion failed'\nnote: run with RUST_BACKTRACE=1\n```\n\n\
               The test expects `Some(42)` but got `None`. \
               The root cause is in the `resolve` function — it returns early when the cache is empty.",
    },
    Reply::Markdown(
        "Sure! Here's a quick summary:\n\n\
         - `goat-protocol` — shared wire types, no TUI deps\n\
         - `goat-core` — session lifecycle and engine trait\n\
         - `goat-tui` — full-screen ratatui interface\n\
         - `goat-code` — binary that wires everything together\n\n\
         The UI and engine communicate **only** through `goat-protocol` types \
         over bounded `tokio::mpsc` channels.",
    ),
    Reply::WithTools {
        tools: &[
            ("write", Some("src/lib.rs"), true),
            ("bash", Some("cargo fmt --all"), true),
        ],
        text: "Done! I wrote the changes and ran the formatter.\n\n\
               ```bash\ncargo fmt --all\ncargo clippy -- -D warnings\n```\n\n\
               Everything passes. The **3 files** I changed:\n\n\
               1. `src/lib.rs` — added the new trait\n\
               2. `src/engine.rs` — implemented it\n\
               3. `tests/integration.rs` — added coverage",
    },
];

enum Reply {
    Markdown(&'static str),
    WithTools {
        tools: &'static [(&'static str, Option<&'static str>, bool)],
        text: &'static str,
    },
}

impl Reply {
    fn text(&self) -> &'static str {
        match self {
            Self::Markdown(t) | Self::WithTools { text: t, .. } => t,
        }
    }
}

pub struct StubEngine;

impl Engine for StubEngine {
    fn spawn(self, ops: mpsc::Receiver<Op>, events: mpsc::Sender<Event>) -> JoinHandle<()> {
        tokio::spawn(run(ops, events))
    }
}

enum Flow {
    Continue,
    Shutdown,
}

async fn run(mut ops: mpsc::Receiver<Op>, events: mpsc::Sender<Event>) {
    tracing::debug!("stub engine started");
    let mut conversation = Conversation::default();
    let mut turn: usize = 0;
    while let Some(op) = ops.recv().await {
        match op {
            Op::SubmitMessage { id, text } => {
                let reply = &REPLIES[turn % REPLIES.len()];
                turn += 1;
                if let Flow::Shutdown =
                    handle_turn(id, text, reply, &mut ops, &events, &mut conversation).await
                {
                    break;
                }
            }
            Op::Interrupt { .. } => {}
            Op::Shutdown => break,
        }
    }
}

async fn handle_turn(
    id: TaskId,
    text: String,
    reply: &Reply,
    ops: &mut mpsc::Receiver<Op>,
    events: &mpsc::Sender<Event>,
    conversation: &mut Conversation,
) -> Flow {
    tracing::debug!(%id, "turn started");
    conversation.push(Message::new(Role::User, text));

    if events.send(Event::TaskStarted { id }).await.is_err() {
        return Flow::Shutdown;
    }

    if let Reply::WithTools { tools, .. } = reply {
        for (name, input, _ok) in *tools {
            let call = ToolCall {
                name: name.to_string(),
                input: input.map(str::to_string),
            };
            if events
                .send(Event::ToolBegin {
                    id,
                    call: call.clone(),
                })
                .await
                .is_err()
            {
                return Flow::Shutdown;
            }
            time::sleep(Duration::from_millis(180)).await;
        }
        for (name, input, ok) in *tools {
            let call = ToolCall {
                name: name.to_string(),
                input: input.map(str::to_string),
            };
            if events
                .send(Event::ToolEnd { id, call, ok: *ok })
                .await
                .is_err()
            {
                return Flow::Shutdown;
            }
        }
    }

    let text = reply.text();

    let (final_text, interrupted, shutdown) = stream_text(id, text, ops, events).await;

    if interrupted {
        let _ = events
            .send(Event::TaskComplete {
                id,
                interrupted: true,
            })
            .await;
    } else {
        conversation.push(Message::new(Role::Agent, final_text));
        let _ = events
            .send(Event::AgentMessage {
                id,
                text: text.to_string(),
            })
            .await;
        let _ = events
            .send(Event::TaskComplete {
                id,
                interrupted: false,
            })
            .await;
    }

    if shutdown {
        Flow::Shutdown
    } else {
        Flow::Continue
    }
}

async fn stream_text(
    id: TaskId,
    text: &str,
    ops: &mut mpsc::Receiver<Op>,
    events: &mpsc::Sender<Event>,
) -> (String, bool, bool) {
    let mut acc = String::new();
    let mut interrupted = false;
    let mut shutdown = false;

    for chunk in text.split_inclusive(' ') {
        tokio::select! {
            biased;
            maybe_op = ops.recv() => match maybe_op {
                Some(Op::Interrupt { .. }) => { interrupted = true; break; }
                Some(Op::Shutdown) | None => { interrupted = true; shutdown = true; break; }
                Some(Op::SubmitMessage { .. }) => {}
            },
            () = time::sleep(DELTA_DELAY) => {
                acc.push_str(chunk);
                if events.send(Event::AgentMessageDelta { id, chunk: chunk.to_owned() }).await.is_err() {
                    interrupted = true;
                    shutdown = true;
                    break;
                }
            }
        }
    }

    (acc, interrupted, shutdown)
}

#[cfg(test)]
mod tests {
    use goat_protocol::{Event, Op, TaskId};

    use crate::{Session, StubEngine};

    #[tokio::test(start_paused = true)]
    async fn streams_a_full_turn() {
        let mut session = Session::spawn(StubEngine);
        session
            .ops()
            .send(Op::SubmitMessage {
                id: TaskId(1),
                text: "hi".into(),
            })
            .await
            .unwrap();

        let mut started = false;
        let mut deltas = 0;
        let mut final_message = false;
        loop {
            match session.next_event().await.expect("engine stopped early") {
                Event::TaskStarted { id } => {
                    assert_eq!(id, TaskId(1));
                    started = true;
                }
                Event::AgentMessageDelta { .. } => deltas += 1,
                Event::AgentMessage { .. } => final_message = true,
                Event::TaskComplete { interrupted, .. } => {
                    assert!(!interrupted);
                    break;
                }
                _ => {}
            }
        }
        assert!(started);
        assert!(deltas > 0);
        assert!(final_message);
    }

    #[tokio::test(start_paused = true)]
    async fn interrupt_ends_turn() {
        let mut session = Session::spawn(StubEngine);
        let ops = session.ops();
        ops.send(Op::SubmitMessage {
            id: TaskId(7),
            text: "hi".into(),
        })
        .await
        .unwrap();
        ops.send(Op::Interrupt { id: TaskId(7) }).await.unwrap();

        loop {
            match session.next_event().await.expect("engine stopped early") {
                Event::TaskComplete { interrupted, id } => {
                    assert_eq!(id, TaskId(7));
                    assert!(interrupted);
                    break;
                }
                Event::AgentMessage { .. } => {
                    panic!("interrupted turn must not finalize a message")
                }
                _ => {}
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn tool_turn_emits_tool_events() {
        let mut session = Session::spawn(StubEngine);
        let ops = session.ops();

        ops.send(Op::SubmitMessage {
            id: TaskId(1),
            text: "first".into(),
        })
        .await
        .unwrap();
        loop {
            if matches!(
                session.next_event().await.expect("engine stopped early"),
                Event::TaskComplete { .. }
            ) {
                break;
            }
        }

        ops.send(Op::SubmitMessage {
            id: TaskId(2),
            text: "use tools".into(),
        })
        .await
        .unwrap();

        let mut tool_begins = 0;
        let mut tool_ends = 0;
        loop {
            match session.next_event().await.expect("engine stopped early") {
                Event::ToolBegin { .. } => tool_begins += 1,
                Event::ToolEnd { .. } => tool_ends += 1,
                Event::TaskComplete { .. } => break,
                _ => {}
            }
        }

        assert!(tool_begins > 0, "expected tool begin events");
        assert_eq!(tool_begins, tool_ends);
    }
}
