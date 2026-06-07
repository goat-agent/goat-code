use std::fmt::Write as _;

use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, path::resolve_in_cwd};
use serde::Deserialize;

pub struct ReadTool;

#[derive(Deserialize)]
struct Input {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "Read"
    }

    fn description(&self) -> &'static str {
        "Read a file from the session directory, returning cat -n style numbered lines. Supports line-based offset and limit."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "offset": {"type": "integer"},
                "limit": {"type": "integer"}
            },
            "required": ["path"]
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
            let max_bytes = ctx.max_output_bytes;
            let truncated = bytes.len() > max_bytes;
            let slice = if truncated {
                &bytes[..max_bytes]
            } else {
                &bytes
            };
            let text = String::from_utf8_lossy(slice);

            let start = args.offset.unwrap_or(1).max(1);
            let mut out = String::new();
            let mut lineno = 0usize;
            let mut emitted = 0usize;
            for line in text.lines() {
                lineno += 1;
                if lineno < start {
                    continue;
                }
                if let Some(limit) = args.limit
                    && emitted >= limit
                {
                    break;
                }
                let _ = writeln!(out, "{lineno:>6}\t{line}");
                emitted += 1;
            }

            if truncated {
                out.push_str("\n[output truncated]\n");
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ReadTool;
    use goat_tool::{Tool, ToolContext, ToolError};

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir).unwrap()
    }

    #[tokio::test]
    async fn reads_numbered_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let ctx = ctx(dir.path());
        let out = ReadTool.run(r#"{"path":"a.txt"}"#, &ctx).await.unwrap();
        assert!(out.contains("     1\tone"));
        assert!(out.contains("     2\ttwo"));
        assert!(out.contains("     3\tthree"));
    }

    #[tokio::test]
    async fn applies_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "1\n2\n3\n4\n5\n").unwrap();
        let ctx = ctx(dir.path());
        let out = ReadTool
            .run(r#"{"path":"a.txt","offset":2,"limit":2}"#, &ctx)
            .await
            .unwrap();
        assert!(out.contains("     2\t2"));
        assert!(out.contains("     3\t3"));
        assert!(!out.contains("     1\t1"));
        assert!(!out.contains("     4\t4"));
    }

    #[tokio::test]
    async fn caps_large_files() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(300 * 1024);
        std::fs::write(dir.path().join("big.txt"), &big).unwrap();
        let ctx = ctx(dir.path());
        let out = ReadTool.run(r#"{"path":"big.txt"}"#, &ctx).await.unwrap();
        assert!(out.contains("[output truncated]"));
    }

    #[tokio::test]
    async fn missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let result = ReadTool.run(r#"{"path":"nope.txt"}"#, &ctx).await;
        assert!(matches!(result, Err(ToolError::NotFound { .. })));
    }
}
