use std::{fmt::Write as _, process::Stdio, time::Duration};

use goat_tool::{Tool, ToolContext, ToolError, ToolFuture};
use serde::Deserialize;
use tokio::{io::AsyncReadExt, process::Command, time};

const MIN_TIMEOUT_MS: u64 = 100;
const MAX_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

pub struct BashTool;

#[derive(Deserialize)]
struct Input {
    command: String,
    timeout_ms: Option<u64>,
}

impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "Bash"
    }

    fn description(&self) -> &'static str {
        "Run a shell command via `sh -c` in the session directory and return its combined output. A nonzero exit code is reported in the output, not as an error."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "timeout_ms": {"type": "integer"}
            },
            "required": ["command"]
        })
    }

    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: Input = serde_json::from_str(input)?;
            let timeout_ms = args
                .timeout_ms
                .unwrap_or(DEFAULT_TIMEOUT_MS)
                .clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS);

            let mut child = Command::new("sh")
                .arg("-c")
                .arg(&args.command)
                .current_dir(&ctx.cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|source| ToolError::Spawn { source })?;

            let mut stdout_pipe = child.stdout.take();
            let mut stderr_pipe = child.stderr.take();

            let collect = async {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(pipe) = stdout_pipe.as_mut() {
                    let _ = pipe.read_to_end(&mut stdout).await;
                }
                if let Some(pipe) = stderr_pipe.as_mut() {
                    let _ = pipe.read_to_end(&mut stderr).await;
                }
                let status = child.wait().await;
                (stdout, stderr, status)
            };

            let result = time::timeout(Duration::from_millis(timeout_ms), collect).await;
            let Ok((stdout, stderr, status)) = result else {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ToolError::Timeout { ms: timeout_ms });
            };

            let code = status.ok().and_then(|s| s.code());
            Ok(build_output(&stdout, &stderr, code, ctx.max_output_bytes))
        })
    }
}

fn build_output(stdout: &[u8], stderr: &[u8], code: Option<i32>, max_bytes: usize) -> String {
    let mut out = String::new();
    out.push_str(&String::from_utf8_lossy(stdout));
    if !stderr.is_empty() {
        out.push_str("\n--- stderr ---\n");
        out.push_str(&String::from_utf8_lossy(stderr));
    }
    if out.len() > max_bytes {
        let boundary = floor_char_boundary(&out, max_bytes);
        out.truncate(boundary);
        out.push_str("\n[output truncated]");
    }
    match code {
        Some(0) | None => {}
        Some(c) => {
            let _ = write!(out, "\nexit code: {c}");
        }
    }
    out
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    let mut boundary = index;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

#[cfg(test)]
mod tests {
    use super::BashTool;
    use goat_tool::{Tool, ToolContext, ToolError};

    fn ctx() -> ToolContext {
        ToolContext::new(&std::env::temp_dir()).unwrap()
    }

    #[tokio::test]
    async fn echoes_stdout() {
        let out = BashTool
            .run(r#"{"command":"echo hello"}"#, &ctx())
            .await
            .unwrap();
        assert!(out.contains("hello"));
    }

    #[tokio::test]
    async fn nonzero_exit_is_ok() {
        let out = BashTool
            .run(r#"{"command":"exit 1"}"#, &ctx())
            .await
            .unwrap();
        assert!(out.contains("exit code: 1"));
    }

    #[tokio::test]
    async fn timeout_errors() {
        let result = BashTool
            .run(r#"{"command":"sleep 999","timeout_ms":100}"#, &ctx())
            .await;
        assert!(matches!(result, Err(ToolError::Timeout { .. })));
    }
}
