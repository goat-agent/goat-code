use std::fmt::Write as _;

use goat_tool::{
    Tool, ToolContext, ToolError, ToolFuture,
    path::{relative_display, resolve_in_cwd},
};
use ignore::{WalkBuilder, overrides::OverrideBuilder};
use serde::Deserialize;

use crate::ignore_error;

pub struct GlobTool;

#[derive(Deserialize)]
struct Input {
    pattern: String,
    path: Option<String>,
}

impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "Glob"
    }

    fn description(&self) -> &'static str {
        "List files in the session directory matching a glob pattern, honoring .gitignore. Results are relative paths, sorted."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "path": {"type": "string"}
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

            let join = tokio::task::spawn_blocking(move || walk(&cwd, &root, &args.pattern)).await;

            match join {
                Ok(result) => result,
                Err(err) => Ok(format!("glob task failed: {err}")),
            }
        })
    }
}

const MAX_GLOB_RESULTS: usize = 1000;

fn walk(cwd: &std::path::Path, root: &std::path::Path, pattern: &str) -> Result<String, ToolError> {
    let mut overrides = OverrideBuilder::new(root);
    overrides.add(pattern).map_err(|err| ignore_error(&err))?;
    let built = overrides.build().map_err(|err| ignore_error(&err))?;

    let mut matches = Vec::new();
    let mut builder = WalkBuilder::new(root);
    builder.require_git(false).overrides(built);
    for entry in builder.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        matches.push(relative_display(cwd, entry.path()));
    }

    if matches.is_empty() {
        return Ok("no files".to_owned());
    }
    matches.sort();
    let total = matches.len();
    matches.truncate(MAX_GLOB_RESULTS);
    let mut out = matches.join("\n");
    if total > MAX_GLOB_RESULTS {
        let _ = write!(
            out,
            "\n[{} more files truncated]\n",
            total - MAX_GLOB_RESULTS
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::GlobTool;
    use goat_tool::{Tool, ToolContext};

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir).unwrap()
    }

    #[tokio::test]
    async fn matches_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::write(dir.path().join("b.rs"), "").unwrap();
        std::fs::write(dir.path().join("c.txt"), "").unwrap();
        let ctx = ctx(dir.path());
        let out = GlobTool.run(r#"{"pattern":"*.rs"}"#, &ctx).await.unwrap();
        assert!(out.contains("a.rs"));
        assert!(out.contains("b.rs"));
        assert!(!out.contains("c.txt"));
    }

    #[tokio::test]
    async fn no_files_reports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        let ctx = ctx(dir.path());
        let out = GlobTool.run(r#"{"pattern":"*.rs"}"#, &ctx).await.unwrap();
        assert_eq!(out, "no files");
    }

    #[tokio::test]
    async fn respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/built.rs"), "").unwrap();
        std::fs::write(dir.path().join("kept.rs"), "").unwrap();
        let ctx = ctx(dir.path());
        let out = GlobTool.run(r#"{"pattern":"*.rs"}"#, &ctx).await.unwrap();
        assert!(out.contains("kept.rs"));
        assert!(!out.contains("built.rs"));
    }
}
