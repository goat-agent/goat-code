use std::fmt::Write as _;

use goat_protocol::ToolDisplay;
use goat_provider::ToolDefinition;
use tokio_util::sync::CancellationToken;

use crate::LoopEnv;

pub(crate) const WEB_SEARCH_TOOL_NAME: &str = "WebSearch";

pub(crate) fn web_search_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: WEB_SEARCH_TOOL_NAME.to_owned(),
        description: "Search the web and return a list of result titles and URLs. Use it to find current information, documentation, or sources; then read the most relevant pages with WebFetch.".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        }),
    }
}

pub(crate) fn web_search_display(input: &str) -> ToolDisplay {
    #[derive(serde::Deserialize)]
    struct Input {
        query: String,
    }
    match serde_json::from_str::<Input>(input) {
        Ok(args) => ToolDisplay::primary(goat_tool::display::flatten(&args.query)),
        Err(_) => goat_tool::display::generic(input),
    }
}

pub(crate) async fn run_web_search(
    env: &LoopEnv<'_>,
    input_json: &str,
    token: &CancellationToken,
) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Input {
        query: String,
    }
    let args: Input = serde_json::from_str(input_json)
        .map_err(|err| format!("invalid WebSearch input: {err}"))?;
    if args.query.trim().is_empty() {
        return Err("query must not be empty".to_owned());
    }
    let handle = env.provider.web_search(args.query);
    let abort = handle.abort_handle();
    let results = tokio::select! {
        biased;
        () = token.cancelled() => {
            abort.abort();
            return Err("interrupted".to_owned());
        }
        joined = handle => joined
            .map_err(|err| format!("web search task failed: {err}"))?
            .map_err(|err| err.to_string())?,
    };
    if results.is_empty() {
        return Ok("No results found.".to_owned());
    }
    let mut out = String::new();
    for (index, result) in results.iter().enumerate() {
        let title = if result.title.is_empty() {
            &result.url
        } else {
            &result.title
        };
        let _ = write!(out, "{}. {title}\n   {}", index + 1, result.url);
        if !result.snippet.is_empty() {
            let _ = write!(out, " · {}", result.snippet);
        }
        out.push('\n');
    }
    Ok(out)
}
