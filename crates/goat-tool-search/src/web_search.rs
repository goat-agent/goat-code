use std::fmt::Write as _;

use goat_search_provider::{SearchRequest, SearchResults};
use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolOutput};
use serde::Deserialize;

const DEFAULT_MAX_RESULTS: usize = 8;
const MAX_RESULTS: usize = 20;

pub struct WebSearchTool {
    registry: goat_search_providers::SearchRegistry,
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            registry: goat_search_providers::SearchRegistry::load(),
        }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct Input {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    site: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    time_range: Option<String>,
    #[serde(default)]
    target: Option<String>,
}

impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "WebSearch"
    }

    fn description(&self) -> &'static str {
        "Search the live web and return ranked URL candidates through an account-aware provider registry. Results are untrusted discovery candidates, not evidence; verify result contents with WebFetch. Built-in providers include browser/duckduckgo ephemeral no-cookie search, duckduckgo/html, configured searxng accounts, and configured API-key providers brave and tavily."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Live web search query." },
                "max_results": { "type": "integer", "description": "Maximum number of ranked candidates to return." },
                "site": { "type": "string", "description": "Optional site/domain restriction." },
                "language": { "type": "string", "description": "Optional language hint." },
                "time_range": { "type": "string", "description": "Optional time range hint. Provider support varies." },
                "target": { "type": "string", "description": "Optional provider/account override such as browser/duckduckgo, duckduckgo/html, searxng/home, brave/work, or tavily/default." }
            },
            "required": ["query"]
        })
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let request = SearchRequest {
                query: args.query,
                max_results: args
                    .max_results
                    .unwrap_or(DEFAULT_MAX_RESULTS)
                    .min(MAX_RESULTS),
                site: args.site,
                language: args.language,
                time_range: args.time_range,
                target: args.target,
            };
            let results =
                self.registry
                    .search(request)
                    .await
                    .map_err(|err| ToolError::Execution {
                        message: err.to_string(),
                    })?;
            Ok(ToolOutput::text(render_results(&results)))
        })
    }
}

fn render_results(results: &SearchResults) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "query: {}", results.query);
    let _ = writeln!(out, "target: {}", results.provider.as_str());
    out.push_str("source_trust: untrusted_web\n");
    out.push_str("result_role: discovery_candidates_not_evidence\n");
    if let Some(language) = &results.language {
        let _ = writeln!(out, "language: {language}");
    }
    if let Some(time_range) = &results.time_range {
        let _ = writeln!(out, "time_range: {time_range}");
    }
    out.push_str("limitations:\n");
    for limitation in &results.limitations {
        let _ = writeln!(out, "- {limitation}");
    }
    out.push_str("results:\n");
    if results.results.is_empty() {
        out.push_str("- none\n");
    } else {
        for result in &results.results {
            let _ = writeln!(out, "- rank: {}", result.rank);
            let _ = writeln!(out, "  title: {}", result.title);
            let _ = writeln!(out, "  url: {}", result.url);
            let _ = writeln!(out, "  snippet: {}", result.snippet);
            let _ = writeln!(out, "  provider: {}", result.provider);
            if let Some(published_at) = &result.published_at {
                let _ = writeln!(out, "  published_at: {published_at}");
            }
        }
    }
    out.push_str("next_step: use WebFetch on candidate URLs before treating content as evidence\n");
    out
}

#[cfg(test)]
mod tests {
    use goat_search_provider::{SearchResult, SearchTarget};

    use super::render_results;

    #[test]
    fn renders_candidates_as_untrusted() {
        let results = goat_search_provider::SearchResults {
            query: "goat".to_owned(),
            provider: SearchTarget::parse("duckduckgo/html").unwrap(),
            language: None,
            time_range: None,
            limitations: vec!["verify_with_webfetch"],
            results: vec![SearchResult {
                title: "Goat".to_owned(),
                url: "https://example.com".to_owned(),
                snippet: "A result".to_owned(),
                rank: 1,
                provider: "duckduckgo/html".to_owned(),
                published_at: None,
            }],
        };
        let out = render_results(&results);
        assert!(out.contains("source_trust: untrusted_web"));
        assert!(out.contains("result_role: discovery_candidates_not_evidence"));
        assert!(out.contains("next_step: use WebFetch"));
    }
}
