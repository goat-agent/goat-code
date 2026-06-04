use std::time::Duration;

use goat_protocol::{Conversation, Event, Message, Op, Role, TaskId};
use tokio::{sync::mpsc, task::JoinHandle, time};

use crate::engine::Engine;

const DELTA_DELAY: Duration = Duration::from_millis(45);

const REPLY: &str = "Got it. This is a stubbed streaming reply from goat-code. When the real \
agent implements the Engine trait, this turn will run actual model and tool calls instead.";

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
    while let Some(op) = ops.recv().await {
        match op {
            Op::SubmitMessage { id, text } => {
                if let Flow::Shutdown =
                    handle_turn(id, text, &mut ops, &events, &mut conversation).await
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
    ops: &mut mpsc::Receiver<Op>,
    events: &mpsc::Sender<Event>,
    conversation: &mut Conversation,
) -> Flow {
    tracing::debug!(%id, "turn started");
    conversation.push(Message::new(Role::User, text));

    if events.send(Event::TaskStarted { id }).await.is_err() {
        return Flow::Shutdown;
    }

    let mut acc = String::new();
    let mut interrupted = false;
    let mut shutdown = false;

    for chunk in REPLY.split_inclusive(' ') {
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
                    return Flow::Shutdown;
                }
            }
        }
    }

    if interrupted {
        let _ = events
            .send(Event::TaskComplete {
                id,
                interrupted: true,
            })
            .await;
    } else {
        conversation.push(Message::new(Role::Agent, acc.clone()));
        let _ = events.send(Event::AgentMessage { id, text: acc }).await;
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
                other => panic!("unexpected event: {other:?}"),
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
}
