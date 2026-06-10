use goat_protocol::ToolDisplay;
use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolOutput, display};

use crate::action::{self, Action};
use crate::session::{self, SessionHandle};

pub struct BrowserTool {
    session: SessionHandle,
}

impl BrowserTool {
    pub fn new(session: SessionHandle) -> Self {
        Self { session }
    }
}

fn exec_err(err: impl std::fmt::Display) -> ToolError {
    ToolError::Execution {
        message: err.to_string(),
    }
}

impl Tool for BrowserTool {
    fn name(&self) -> &'static str {
        "Browser"
    }

    fn description(&self) -> &'static str {
        "Drive a real Chrome window to browse the web. The first action opens a visible Chrome window with a persistent profile, so logins survive across sessions: if a page shows a login wall, ask the user to sign in manually in that window, then continue. There is a single shared browser window with one active page - refs and page state are global, so re-snapshot whenever the page may have changed. Most actions return a fresh accessibility snapshot of the page - an indented tree of interactive elements, each tagged with a ref like e12. Refer to elements by that ref. Refs are only valid until the next snapshot or navigation. Use the screenshot action when you need to see the page visually."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["navigate","snapshot","click","type","select","press_key","evaluate","screenshot","close"],
                    "description": "The action to perform."
                },
                "url": { "type": "string", "description": "URL for action=navigate (scheme optional, defaults to https)." },
                "ref": { "type": "string", "description": "Element ref like e12 from the latest snapshot, for click/type/select." },
                "text": { "type": "string", "description": "Text to type, for action=type." },
                "submit": { "type": "boolean", "description": "Press Enter after typing, for action=type." },
                "value": { "type": "string", "description": "Option value or visible label to choose, for action=select." },
                "key": { "type": "string", "description": "Key name to press, e.g. Enter, Escape, ArrowDown, Tab, for action=press_key." },
                "js": { "type": "string", "description": "JavaScript to evaluate in the page, for action=evaluate. Use for scrolling, extraction, waiting, history navigation." }
            },
            "required": ["action"]
        })
    }

    fn display_input(&self, input: &str) -> ToolDisplay {
        let Ok(action) = action::parse(input) else {
            return display::generic(input);
        };
        match action {
            Action::Navigate { url } => ToolDisplay::with_detail("navigate", url),
            Action::Snapshot => ToolDisplay::primary("snapshot"),
            Action::Click { reference } => ToolDisplay::with_detail("click", reference),
            Action::Type {
                reference, text, ..
            } => ToolDisplay::with_detail(
                "type",
                format!("{reference} · {}", display::flatten(&text)),
            ),
            Action::Select { reference, value } => {
                ToolDisplay::with_detail("select", format!("{reference} · {value}"))
            }
            Action::PressKey { key } => ToolDisplay::with_detail("press key", key),
            Action::Evaluate { js } => ToolDisplay::with_detail("evaluate", display::flatten(&js)),
            Action::Screenshot => ToolDisplay::primary("screenshot"),
            Action::Close => ToolDisplay::primary("close"),
        }
    }

    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let action = action::parse(input).map_err(exec_err)?;
            let mut guard = self.session.lock().await;
            if matches!(action, Action::Close) {
                return Ok(ToolOutput::text(session::close(&mut guard).await));
            }
            let session = session::ensure_session(&mut guard)
                .await
                .map_err(exec_err)?;
            session
                .dispatch(action, ctx.max_output_bytes)
                .await
                .map_err(exec_err)
        })
    }
}
