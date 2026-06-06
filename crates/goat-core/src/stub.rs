use std::time::Duration;

use goat_protocol::{
    Conversation, Event, Message, Op, Role, TaskId, ToolCall, ToolCallId, ToolOutcome,
};
use tokio::{sync::mpsc, task::JoinHandle, time};

use crate::engine::Engine;

const DELTA_DELAY: Duration = Duration::from_millis(45);

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

enum Halt {
    Interrupted,
    Shutdown,
}

impl Halt {
    fn into_flow(self) -> Flow {
        match self {
            Halt::Interrupted => Flow::Continue,
            Halt::Shutdown => Flow::Shutdown,
        }
    }
}

struct Step {
    text: &'static str,
    tool_name: &'static str,
    tool_input: &'static str,
    tool_delay: Duration,
    tool_outcome: ToolOutcome,
}

const STEPS: &[Step] = &[
    Step {
        text: "Let me read the source. ",
        tool_name: "Read",
        tool_input: "crates/goat-core/src/stub.rs",
        tool_delay: Duration::from_millis(350),
        tool_outcome: ToolOutcome {
            ok: true,
            summary: None,
        },
    },
    Step {
        text: "Found a minor issue. Patching it. ",
        tool_name: "Edit",
        tool_input: "crates/goat-core/src/stub.rs",
        tool_delay: Duration::from_millis(400),
        tool_outcome: ToolOutcome {
            ok: true,
            summary: Some(String::new()),
        },
    },
    Step {
        text: "Let me verify the build. ",
        tool_name: "Bash",
        tool_input: "cargo build --workspace",
        tool_delay: Duration::from_millis(600),
        tool_outcome: ToolOutcome {
            ok: true,
            summary: None,
        },
    },
];

const FINAL_TEXT: &str =
    "Build passed. When the real agent implements the Engine trait, this will use live output.";

async fn run(mut ops: mpsc::Receiver<Op>, events: mpsc::Sender<Event>) {
    tracing::debug!("stub engine started");
    let mut conversation = Conversation::default();
    let mut next_call_id = 1u64;
    while let Some(op) = ops.recv().await {
        match op {
            Op::SubmitMessage { id, text } => {
                if let Flow::Shutdown = handle_turn(
                    id,
                    text,
                    &mut ops,
                    &events,
                    &mut conversation,
                    &mut next_call_id,
                )
                .await
                {
                    break;
                }
            }
            Op::Interrupt { .. }
            | Op::SelectModel { .. }
            | Op::RefreshModels
            | Op::Login { .. } => {}
            Op::Shutdown => break,
        }
    }
}

async fn stream_text(
    text: &str,
    acc: &mut String,
    task_id: TaskId,
    ops: &mut mpsc::Receiver<Op>,
    events: &mpsc::Sender<Event>,
) -> Option<Halt> {
    for chunk in text.split_inclusive(' ') {
        tokio::select! {
            biased;
            maybe_op = ops.recv() => match maybe_op {
                Some(Op::Interrupt { .. }) => return Some(Halt::Interrupted),
                Some(Op::Shutdown) | None => return Some(Halt::Shutdown),
                Some(
                    Op::SubmitMessage { .. }
                    | Op::SelectModel { .. }
                    | Op::RefreshModels
                    | Op::Login { .. },
                ) => {}
            },
            () = time::sleep(DELTA_DELAY) => {
                acc.push_str(chunk);
                if events.send(Event::TextDelta { id: task_id, chunk: chunk.to_owned() }).await.is_err() {
                    return Some(Halt::Shutdown);
                }
            }
        }
    }
    None
}

async fn run_tool(
    task_id: TaskId,
    cid: ToolCallId,
    step: &Step,
    ops: &mut mpsc::Receiver<Op>,
    events: &mpsc::Sender<Event>,
) -> Option<Halt> {
    if events
        .send(Event::ToolStarted {
            id: task_id,
            call: ToolCall {
                id: cid,
                name: step.tool_name.to_owned(),
                input: step.tool_input.to_owned(),
            },
        })
        .await
        .is_err()
    {
        return Some(Halt::Shutdown);
    }

    let halt = tokio::select! {
        biased;
        maybe_op = ops.recv() => match maybe_op {
            Some(Op::Interrupt { .. }) => Some(Halt::Interrupted),
            Some(Op::Shutdown) | None => Some(Halt::Shutdown),
            Some(
                Op::SubmitMessage { .. }
                | Op::SelectModel { .. }
                | Op::RefreshModels
                | Op::Login { .. },
            ) => None,
        },
        () = time::sleep(step.tool_delay) => None,
    };

    if halt.is_some() {
        return halt;
    }

    if events
        .send(Event::ToolDone {
            id: task_id,
            call: cid,
            outcome: step.tool_outcome.clone(),
        })
        .await
        .is_err()
    {
        return Some(Halt::Shutdown);
    }

    None
}

async fn handle_turn(
    task_id: TaskId,
    text: String,
    ops: &mut mpsc::Receiver<Op>,
    events: &mpsc::Sender<Event>,
    conversation: &mut Conversation,
    next_call_id: &mut u64,
) -> Flow {
    tracing::debug!(%task_id, "turn started");
    conversation.push(Message::new(Role::User, text));
    if events
        .send(Event::TaskStarted { id: task_id })
        .await
        .is_err()
    {
        return Flow::Shutdown;
    }

    let bail = |halt: Halt| async move {
        let _ = events
            .send(Event::TaskDone {
                id: task_id,
                interrupted: true,
            })
            .await;
        halt.into_flow()
    };

    for step in STEPS {
        let mut acc = String::new();
        if let Some(halt) = stream_text(step.text, &mut acc, task_id, ops, events).await {
            return bail(halt).await;
        }
        if events
            .send(Event::TextDone {
                id: task_id,
                text: acc,
            })
            .await
            .is_err()
        {
            return Flow::Shutdown;
        }
        let cid = ToolCallId({
            let id = *next_call_id;
            *next_call_id += 1;
            id
        });
        if let Some(halt) = run_tool(task_id, cid, step, ops, events).await {
            return bail(halt).await;
        }
    }

    let mut acc = String::new();
    if let Some(halt) = stream_text(FINAL_TEXT, &mut acc, task_id, ops, events).await {
        return bail(halt).await;
    }
    conversation.push(Message::new(Role::Agent, acc.clone()));
    let _ = events
        .send(Event::TextDone {
            id: task_id,
            text: acc,
        })
        .await;
    let _ = events
        .send(Event::TaskDone {
            id: task_id,
            interrupted: false,
        })
        .await;
    Flow::Continue
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
        let mut deltas = 0u32;
        let mut text_dones = 0u32;
        let mut tool_starts = 0u32;
        let mut tool_dones = 0u32;
        loop {
            match session.next_event().await.expect("engine stopped early") {
                Event::TaskStarted { id } => {
                    assert_eq!(id, TaskId(1));
                    started = true;
                }
                Event::TextDelta { .. } => deltas += 1,
                Event::TextDone { .. } => text_dones += 1,
                Event::ToolStarted { .. } => tool_starts += 1,
                Event::ToolDone { outcome, .. } => {
                    assert!(outcome.ok);
                    tool_dones += 1;
                }
                Event::TaskDone { interrupted, .. } => {
                    assert!(!interrupted);
                    break;
                }
                other @ (Event::Error { .. }
                | Event::ModelListChanged { .. }
                | Event::ModelSelected { .. }
                | Event::LoginProviders { .. }
                | Event::LoginStatus { .. }) => panic!("unexpected event: {other:?}"),
            }
        }

        assert!(started);
        assert!(deltas > 0);
        assert_eq!(text_dones, 4);
        assert_eq!(tool_starts, 3);
        assert_eq!(tool_dones, 3);
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
                Event::TaskDone { interrupted, id } => {
                    assert_eq!(id, TaskId(7));
                    assert!(interrupted);
                    break;
                }
                Event::TextDone { .. } => panic!("interrupted turn must not finalize text"),
                _ => {}
            }
        }
    }
}
