use std::{fmt::Write as _, process::Stdio, time::Duration};

use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolOutput};
use serde::Deserialize;
use tokio::{io::AsyncReadExt, process::Command, time};

const MIN_TIMEOUT_MS: u64 = 100;
const MAX_TIMEOUT_MS: u64 = 300_000;

struct ChildGuard(tokio::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
    }
}

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
            let timeout_dur = match args.timeout_ms {
                Some(ms) => Duration::from_millis(ms.clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS)),
                None => ctx.bash_timeout,
            };

            let child = Command::new("sh")
                .arg("-c")
                .arg(&args.command)
                .current_dir(&ctx.cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|source| ToolError::Spawn { source })?;

            let mut guard = ChildGuard(child);
            let mut stdout_pipe = guard.0.stdout.take();
            let mut stderr_pipe = guard.0.stderr.take();

            let result = time::timeout(timeout_dur, async {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                let (stdout_result, stderr_result) = tokio::join!(
                    async {
                        if let Some(pipe) = stdout_pipe.as_mut() {
                            pipe.read_to_end(&mut stdout).await
                        } else {
                            Ok(0)
                        }
                    },
                    async {
                        if let Some(pipe) = stderr_pipe.as_mut() {
                            pipe.read_to_end(&mut stderr).await
                        } else {
                            Ok(0)
                        }
                    }
                );
                let _ = stdout_result;
                let _ = stderr_result;
                let status = guard.0.wait().await;
                (stdout, stderr, status)
            })
            .await;

            let Ok((stdout, stderr, status)) = result else {
                return Err(ToolError::Timeout {
                    ms: u64::try_from(timeout_dur.as_millis()).unwrap_or(MAX_TIMEOUT_MS),
                });
            };

            let code = status.ok().and_then(|s| s.code());
            Ok(ToolOutput::text(build_output(
                &stdout,
                &stderr,
                code,
                ctx.max_output_bytes,
            )))
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
        let boundary = out.floor_char_boundary(max_bytes);
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
        assert!(out.as_text().unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn nonzero_exit_is_ok() {
        let out = BashTool
            .run(r#"{"command":"exit 1"}"#, &ctx())
            .await
            .unwrap();
        assert!(out.as_text().unwrap().contains("exit code: 1"));
    }

    #[tokio::test]
    async fn timeout_errors() {
        let result = BashTool
            .run(r#"{"command":"sleep 999","timeout_ms":100}"#, &ctx())
            .await;
        assert!(matches!(result, Err(ToolError::Timeout { .. })));
    }
}
