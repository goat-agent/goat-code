use std::fmt::Write as _;

use goat_tool::{
    Tool, ToolContext, ToolError, ToolFuture,
    path::{relative_display, resolve_in_cwd},
};
use ignore::{WalkBuilder, overrides::OverrideBuilder};
use regex::Regex;
use serde::Deserialize;

use crate::ignore_error;

const DEFAULT_MAX_RESULTS: usize = 100;

pub struct GrepTool;

#[derive(Deserialize)]
struct Input {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    max_results: Option<usize>,
}

impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "Grep"
    }

    fn description(&self) -> &'static str {
        "Search file contents for a regular expression across the session directory, honoring .gitignore. Optionally restrict to a subpath or glob."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "path": {"type": "string"},
                "glob": {"type": "string"},
                "max_results": {"type": "integer"}
            },
            "required": ["pattern"]
        })
    }

    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let root = match &args.path {
                Some(path) => resolve_in_cwd(&ctx.cwd, path)?,
                None => ctx.cwd.clone(),
            };
            let cwd = ctx.cwd.clone();
            let max_results = args.max_results.unwrap_or(DEFAULT_MAX_RESULTS);
            let max_output_bytes = ctx.max_output_bytes;

            let join = tokio::task::spawn_blocking(move || {
                search(
                    &cwd,
                    &root,
                    &args.pattern,
                    args.glob.as_deref(),
                    max_results,
                    max_output_bytes,
                )
            })
            .await;

            match join {
                Ok(result) => result,
                Err(err) => Ok(format!("search task failed: {err}")),
            }
        })
    }
}

fn search(
    cwd: &std::path::Path,
    root: &std::path::Path,
    pattern: &str,
    glob: Option<&str>,
    max_results: usize,
    max_output_bytes: usize,
) -> Result<String, ToolError> {
    let regex = Regex::new(pattern)?;
    let mut builder = WalkBuilder::new(root);
    builder.require_git(false);
    if let Some(glob) = glob {
        let mut overrides = OverrideBuilder::new(root);
        overrides.add(glob).map_err(|err| ignore_error(&err))?;
        let built = overrides.build().map_err(|err| ignore_error(&err))?;
        builder.overrides(built);
    }

    let mut out = String::new();
    let mut count = 0usize;
    let mut capped = false;

    'walk: for entry in builder.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let Ok(contents) = std::fs::read(entry.path()) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&contents) else {
            continue;
        };
        let display = relative_display(cwd, entry.path());
        for (lineno, line) in text.lines().enumerate() {
            if regex.is_match(line) {
                if count >= max_results || out.len() >= max_output_bytes {
                    capped = true;
                    break 'walk;
                }
                let _ = writeln!(out, "{display}:{}: {line}", lineno + 1);
                count += 1;
            }
        }
    }

    if count == 0 {
        return Ok("no matches".to_owned());
    }
    if capped {
        out.push_str("\n[output truncated]");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::GrepTool;
    use goat_tool::{Tool, ToolContext};

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir).unwrap()
    }

    #[tokio::test]
    async fn finds_matching_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\nbeta needle\ngamma\n").unwrap();
        let ctx = ctx(dir.path());
        let out = GrepTool.run(r#"{"pattern":"needle"}"#, &ctx).await.unwrap();
        assert!(out.contains("a.txt:2: beta needle"));
    }

    #[tokio::test]
    async fn no_match_reports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "nothing here\n").unwrap();
        let ctx = ctx(dir.path());
        let out = GrepTool.run(r#"{"pattern":"absent"}"#, &ctx).await.unwrap();
        assert_eq!(out, "no matches");
    }

    #[tokio::test]
    async fn respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "skipped/\n").unwrap();
        std::fs::create_dir(dir.path().join("skipped")).unwrap();
        std::fs::write(dir.path().join("skipped/a.txt"), "needle\n").unwrap();
        std::fs::write(dir.path().join("kept.txt"), "needle\n").unwrap();
        let ctx = ctx(dir.path());
        let out = GrepTool.run(r#"{"pattern":"needle"}"#, &ctx).await.unwrap();
        assert!(out.contains("kept.txt"));
        assert!(!out.contains("skipped"));
    }
}
