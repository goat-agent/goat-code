use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolOutput, path::resolve_in_cwd};
use serde::Deserialize;

use crate::tools::relative_display;

pub struct WriteTool;

#[derive(Deserialize)]
struct Input {
    path: String,
    content: String,
}

impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "Write"
    }

    fn description(&self) -> &'static str {
        "Write content to a file in the session directory, creating parent directories and overwriting any existing file."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["path", "content"]
        })
    }

    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let resolved = resolve_in_cwd(&ctx.cwd, &args.path)?;
            if let Some(parent) = resolved.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|source| ToolError::Io {
                        path: args.path.clone(),
                        source,
                    })?;
            }
            let bytes = args.content.len();
            tokio::fs::write(&resolved, args.content.as_bytes())
                .await
                .map_err(|source| ToolError::Io {
                    path: args.path.clone(),
                    source,
                })?;
            let rel = relative_display(&ctx.cwd, &resolved);
            Ok(ToolOutput::text(format!("wrote {bytes} bytes to {rel}")))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::WriteTool;
    use goat_tool::{Tool, ToolContext, ToolError};

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir).unwrap()
    }

    #[tokio::test]
    async fn creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let out = WriteTool
            .run(r#"{"path":"out.txt","content":"hello"}"#, &ctx)
            .await
            .unwrap();
        assert!(out.as_text().unwrap().contains("wrote 5 bytes to out.txt"));
        let written = std::fs::read_to_string(dir.path().join("out.txt")).unwrap();
        assert_eq!(written, "hello");
    }

    #[tokio::test]
    async fn creates_nested_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        WriteTool
            .run(r#"{"path":"a/b/c.txt","content":"x"}"#, &ctx)
            .await
            .unwrap();
        assert!(dir.path().join("a/b/c.txt").exists());
    }

    #[tokio::test]
    async fn escape_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let result = WriteTool
            .run(r#"{"path":"../evil.txt","content":"x"}"#, &ctx)
            .await;
        assert!(matches!(result, Err(ToolError::PathEscape { .. })));
    }
}
