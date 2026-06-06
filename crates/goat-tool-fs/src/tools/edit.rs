use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, path::resolve_in_cwd};
use serde::Deserialize;

use crate::tools::relative_display;

pub struct EditTool;

#[derive(Deserialize)]
struct Input {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "Edit"
    }

    fn description(&self) -> &'static str {
        "Replace occurrences of old_string with new_string in a file. By default requires a single unique match; set replace_all to replace every occurrence."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "old_string": {"type": "string"},
                "new_string": {"type": "string"},
                "replace_all": {"type": "boolean"}
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let resolved = resolve_in_cwd(&ctx.cwd, &args.path)?;
            if !resolved.exists() {
                return Err(ToolError::NotFound { path: args.path });
            }
            let bytes = tokio::fs::read(&resolved)
                .await
                .map_err(|source| ToolError::Io {
                    path: args.path.clone(),
                    source,
                })?;
            let contents = String::from_utf8_lossy(&bytes).into_owned();
            let count = contents.matches(&args.old_string).count();
            if count == 0 {
                return Err(ToolError::EditNoMatch { path: args.path });
            }
            if !args.replace_all && count > 1 {
                return Err(ToolError::EditNotUnique { path: args.path });
            }
            let updated = if args.replace_all {
                contents.replace(&args.old_string, &args.new_string)
            } else {
                contents.replacen(&args.old_string, &args.new_string, 1)
            };
            tokio::fs::write(&resolved, updated.as_bytes())
                .await
                .map_err(|source| ToolError::Io {
                    path: args.path.clone(),
                    source,
                })?;
            let rel = relative_display(&ctx.cwd, &resolved);
            Ok(format!("replaced {count} occurrence(s) in {rel}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::EditTool;
    use goat_tool::{Tool, ToolContext, ToolError};

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir).unwrap()
    }

    #[tokio::test]
    async fn replaces_single() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello world").unwrap();
        let ctx = ctx(dir.path());
        let out = EditTool
            .run(
                r#"{"path":"a.txt","old_string":"world","new_string":"there"}"#,
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.contains("replaced 1 occurrence(s) in a.txt"));
        let result = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert_eq!(result, "hello there");
    }

    #[tokio::test]
    async fn no_match_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let ctx = ctx(dir.path());
        let result = EditTool
            .run(
                r#"{"path":"a.txt","old_string":"absent","new_string":"x"}"#,
                &ctx,
            )
            .await;
        assert!(matches!(result, Err(ToolError::EditNoMatch { .. })));
    }

    #[tokio::test]
    async fn not_unique_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a a a").unwrap();
        let ctx = ctx(dir.path());
        let result = EditTool
            .run(
                r#"{"path":"a.txt","old_string":"a","new_string":"b"}"#,
                &ctx,
            )
            .await;
        assert!(matches!(result, Err(ToolError::EditNotUnique { .. })));
    }

    #[tokio::test]
    async fn replace_all_replaces_every() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a a a").unwrap();
        let ctx = ctx(dir.path());
        let out = EditTool
            .run(
                r#"{"path":"a.txt","old_string":"a","new_string":"b","replace_all":true}"#,
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.contains("replaced 3 occurrence(s)"));
        let result = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert_eq!(result, "b b b");
    }
}
