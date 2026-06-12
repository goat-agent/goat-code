use goat_protocol::{Event, ToolCall, ToolCallId, ToolDisplay, ToolOutcome};
use goat_provider::{ContentBlock, Provider, ToolDefinition};
use goat_tool::{ToolContent, ToolContext, ToolOutput};
use goat_tools::ToolRegistry;
use tokio_util::sync::CancellationToken;

use crate::{
    Ctx, LoopEnv, Run,
    agent::ToolSelection,
    ask::{ASK_TOOL_NAME, ask_call_display, ask_tool_def, run_ask},
    delegate::{AGENT_TOOL_NAME, agent_call_display, agent_tool_def, run_delegation},
    persist::{create_tool_call_record, finish_tool_db},
};

pub(crate) struct ToolExecResult {
    result_content: ContentBlock,
    cancelled: bool,
}

pub(crate) struct ToolBatchResult {
    pub(crate) tool_results: Vec<ContentBlock>,
    pub(crate) cancelled: bool,
}

struct Prepared<'a> {
    vendor_id: &'a str,
    name: &'a str,
    input_json: &'a str,
    tui_id: u64,
    db_id: Option<i64>,
}

pub(crate) fn tool_outcome(result: &Result<ToolOutput, String>) -> ToolOutcome {
    match result {
        Ok(output) => ToolOutcome {
            ok: true,
            summary: output.summary.clone(),
        },
        Err(message) => ToolOutcome {
            ok: false,
            summary: Some(message.clone()),
        },
    }
}

pub(crate) fn call_display(tools: &ToolRegistry, name: &str, input: &str) -> ToolDisplay {
    match name {
        AGENT_TOOL_NAME => agent_call_display(input),
        ASK_TOOL_NAME => ask_call_display(input),
        _ => tools.get(name).map_or_else(
            || goat_tool::display::generic(input),
            |tool| tool.display_input(input),
        ),
    }
}

pub(crate) fn summarize_line(text: &str) -> Option<String> {
    let line = text.lines().find(|line| !line.trim().is_empty())?;
    let flat = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 80 {
        let head: String = flat.chars().take(80).collect();
        Some(format!("{head}…"))
    } else {
        Some(flat)
    }
}

async fn run_regular_tool(
    ctx: &Ctx<'_>,
    name: &str,
    input_json: &str,
    tool_ctx: &ToolContext,
    token: &CancellationToken,
) -> Option<Result<ToolOutput, String>> {
    let fut = async {
        match ctx.tools.get(name) {
            Some(tool) => tool
                .run(input_json, tool_ctx)
                .await
                .map_err(|err| err.to_string()),
            None => Err(format!("unknown tool: {name}")),
        }
    };
    let mut fut = std::pin::pin!(fut);
    tokio::select! {
        biased;
        () = token.cancelled() => None,
        result = &mut fut => Some(result),
    }
}

const MAX_TOOL_RESULT_BYTES: usize = 64 * 1024;

pub(crate) fn cap_tool_result(mut content: String) -> String {
    if content.len() > MAX_TOOL_RESULT_BYTES {
        let boundary = content.floor_char_boundary(MAX_TOOL_RESULT_BYTES);
        content.truncate(boundary);
        content.push_str("\n[output truncated]\n");
    }
    content
}

async fn execute_tool(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    env: &LoopEnv<'_>,
    prep: &Prepared<'_>,
    tool_ctx: &ToolContext,
    token: &CancellationToken,
) -> ToolExecResult {
    let step: Option<Result<ToolOutput, String>> =
        if prep.name == ASK_TOOL_NAME && env.allow_delegate {
            Some(
                run_ask(ctx, run, prep.input_json, ToolCallId(prep.tui_id), token)
                    .await
                    .map(ToolOutput::text),
            )
        } else if prep.name == AGENT_TOOL_NAME && env.allow_delegate {
            match ctx.semaphore.acquire().await {
                Ok(_permit) if !token.is_cancelled() => Some(
                    run_delegation(ctx, env, prep.input_json, run.id, token)
                        .await
                        .map(ToolOutput::text),
                ),
                _ => None,
            }
        } else {
            run_regular_tool(ctx, prep.name, prep.input_json, tool_ctx, token).await
        };
    let Some(result) = step else {
        let outcome = ToolOutcome {
            ok: false,
            summary: Some("interrupted".to_owned()),
        };
        finish_tool_db(ctx, prep.db_id, &outcome).await;
        let _ = ctx
            .events
            .send(Event::ToolDone {
                id: run.id,
                call: ToolCallId(prep.tui_id),
                outcome,
            })
            .await;
        return ToolExecResult {
            result_content: ContentBlock::text_result(prep.vendor_id, "interrupted", true),
            cancelled: true,
        };
    };
    let outcome = tool_outcome(&result);
    let is_error = !outcome.ok;
    finish_tool_db(ctx, prep.db_id, &outcome).await;
    let _ = ctx
        .events
        .send(Event::ToolDone {
            id: run.id,
            call: ToolCallId(prep.tui_id),
            outcome,
        })
        .await;
    let content = match result {
        Ok(output) => match output.content {
            ToolContent::Text(text) => {
                vec![ContentBlock::Text {
                    text: cap_tool_result(text),
                }]
            }
            ToolContent::Image(img) => {
                vec![ContentBlock::Image {
                    media_type: img.media_type,
                    data: img.data,
                }]
            }
        },
        Err(msg) => vec![ContentBlock::Text { text: msg }],
    };
    ToolExecResult {
        result_content: ContentBlock::ToolResult {
            tool_use_id: prep.vendor_id.to_owned(),
            content,
            is_error,
        },
        cancelled: false,
    }
}

pub(crate) async fn run_tool_batch(
    ctx: &Ctx<'_>,
    run: &Run<'_>,
    env: &LoopEnv<'_>,
    pending_calls: &[(String, String, String)],
    call_seq: &mut u64,
    tool_ctx: &ToolContext,
    token: &CancellationToken,
) -> ToolBatchResult {
    let mut prepared: Vec<Prepared> = Vec::with_capacity(pending_calls.len());
    for (vendor_id, name, input_json) in pending_calls {
        *call_seq += 1;
        let tui_id = *call_seq;
        let _ = ctx
            .events
            .send(Event::ToolStarted {
                id: run.id,
                call: ToolCall {
                    id: ToolCallId(tui_id),
                    name: name.clone(),
                    display: call_display(ctx.tools, name, input_json),
                },
            })
            .await;
        let db_id = match run.ids() {
            Some(ids) => create_tool_call_record(ctx, ids, vendor_id, name, input_json).await,
            None => None,
        };
        prepared.push(Prepared {
            vendor_id: vendor_id.as_str(),
            name: name.as_str(),
            input_json: input_json.as_str(),
            tui_id,
            db_id,
        });
    }
    let results = futures::future::join_all(
        prepared
            .iter()
            .map(|prep| execute_tool(ctx, run, env, prep, tool_ctx, token)),
    )
    .await;
    let mut tool_results = Vec::with_capacity(results.len());
    let mut cancelled = false;
    for result in results {
        if result.cancelled {
            cancelled = true;
        }
        tool_results.push(result.result_content);
    }
    ToolBatchResult {
        tool_results,
        cancelled,
    }
}

pub(crate) fn build_tool_defs(
    ctx: &Ctx<'_>,
    provider: &dyn Provider,
    selection: Option<&ToolSelection>,
    allow_delegate: bool,
) -> Vec<ToolDefinition> {
    if !provider.capabilities().tools {
        return Vec::new();
    }
    let mut defs: Vec<ToolDefinition> = ctx
        .tools
        .specs()
        .into_iter()
        .filter(|spec| selection.is_none_or(|sel| sel.allows(spec.name)))
        .map(|spec| ToolDefinition {
            name: spec.name.to_owned(),
            description: spec.description.to_owned(),
            input_schema: spec.parameters,
        })
        .collect();
    if allow_delegate {
        if !ctx.agents.is_empty() {
            defs.push(agent_tool_def(ctx));
        }
        defs.push(ask_tool_def());
    }
    defs
}
