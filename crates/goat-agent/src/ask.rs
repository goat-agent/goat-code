use goat_protocol::{AskQuestion, Event, ToolCallId, ToolDisplay};
use goat_provider::ToolDefinition;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::{Ctx, Run};

pub(crate) const ASK_TOOL_NAME: &str = "Ask";

pub(crate) fn ask_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: ASK_TOOL_NAME.to_owned(),
        description: "Pause execution and ask the user one or more questions, each with optional choice options. Returns the user's answers as a JSON array of strings in the same order as the questions. Use when you need the user's input or a decision before proceeding.".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string" },
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" },
                                        "description": { "type": "string" }
                                    },
                                    "required": ["label"]
                                }
                            }
                        },
                        "required": ["question"]
                    }
                }
            },
            "required": ["questions"]
        }),
    }
}

pub(crate) fn ask_call_display(input: &str) -> ToolDisplay {
    #[derive(serde::Deserialize)]
    struct Input {
        questions: Vec<AskQuestion>,
    }
    let Ok(args) = serde_json::from_str::<Input>(input) else {
        return goat_tool::display::generic(input);
    };
    let Some(first) = args.questions.first() else {
        return goat_tool::display::generic(input);
    };
    let primary = goat_tool::display::flatten(&first.question);
    if args.questions.len() > 1 {
        ToolDisplay::with_detail(primary, format!("+{} more", args.questions.len() - 1))
    } else {
        ToolDisplay::primary(primary)
    }
}

pub(crate) async fn run_ask(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    input_json: &str,
    call_id: ToolCallId,
    token: &CancellationToken,
) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Input {
        questions: Vec<AskQuestion>,
    }
    let args: Input =
        serde_json::from_str(input_json).map_err(|err| format!("invalid Ask input: {err}"))?;
    if args.questions.is_empty() {
        return Err("questions must not be empty".to_owned());
    }
    let (tx, rx) = oneshot::channel::<Vec<String>>();
    ctx.asks.lock().await.insert(call_id, tx);
    let _ = ctx
        .events
        .send(Event::AskStarted {
            id: run.id,
            call: call_id,
            questions: args.questions,
        })
        .await;
    let result = tokio::select! {
        biased;
        () = token.cancelled() => {
            ctx.asks.lock().await.remove(&call_id);
            let _ = ctx
                .events
                .send(Event::AskDismissed { id: run.id, call: call_id })
                .await;
            return Err("interrupted".to_owned());
        }
        res = rx => res,
    };
    match result {
        Ok(answers) => {
            serde_json::to_string(&answers).map_err(|err| format!("serialize error: {err}"))
        }
        Err(_) => Err("answer channel closed".to_owned()),
    }
}
