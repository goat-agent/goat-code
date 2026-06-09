use std::sync::Arc;

use goat_tool::{Tool, ToolContext, ToolError, ToolFuture, ToolImage, ToolOutput};

use crate::{
    action::{self, Action},
    backend::ComputerBackend,
};

pub struct ComputerTool {
    pub(crate) backend: Arc<dyn ComputerBackend>,
}

impl ComputerTool {
    pub fn new(backend: Arc<dyn ComputerBackend>) -> Self {
        Self { backend }
    }

    pub fn display_size(&self) -> (u32, u32) {
        self.backend.display_size()
    }
}

fn exec_err(msg: impl std::fmt::Display) -> ToolError {
    ToolError::Execution {
        message: msg.to_string(),
    }
}

impl Tool for ComputerTool {
    fn name(&self) -> &'static str {
        "Computer"
    }

    fn description(&self) -> &'static str {
        "Control the local desktop: take screenshots and drive the mouse and keyboard. Coordinates are pixels on the screenshot you last received (origin top-left). Always take a screenshot first to see the screen, and again after acting to confirm the result."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["screenshot","move","click","double_click","triple_click",
                             "mouse_down","mouse_up","drag","scroll","type","key","hold_key","wait","zoom"],
                    "description": "The action to perform."
                },
                "x": { "type": "integer", "description": "X pixel for move/click/scroll." },
                "y": { "type": "integer", "description": "Y pixel for move/click/scroll." },
                "button": { "type": "string", "enum": ["left","right","middle"], "description": "Mouse button for click (default left)." },
                "modifiers": { "type": "array", "items": { "type": "string" }, "description": "Held modifier keys, e.g. [\"ctrl\",\"shift\"]." },
                "dx": { "type": "integer", "description": "Horizontal scroll amount (+right/-left)." },
                "dy": { "type": "integer", "description": "Vertical scroll amount (+up/-down)." },
                "path": { "type": "array", "items": { "type": "object", "properties": { "x": {"type":"integer"}, "y": {"type":"integer"} } }, "description": "Drag path of {x,y} points; first is press, last is release." },
                "text": { "type": "string", "description": "Text to type." },
                "keys": { "type": "array", "items": { "type": "string" }, "description": "Key chord for action=key, e.g. [\"ctrl\",\"c\"] or [\"enter\"]." },
                "key": { "type": "string", "description": "Single key for hold_key." },
                "ms": { "type": "integer", "description": "Duration in milliseconds for wait/hold_key." },
                "x1": { "type": "integer" }, "y1": { "type": "integer" },
                "x2": { "type": "integer" }, "y2": { "type": "integer" }
            },
            "required": ["action"]
        })
    }

    fn run<'a>(&'a self, input: &'a str, _ctx: &'a ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let action = action::parse(input).map_err(exec_err)?;
            let backend = self.backend.clone();

            let img = tokio::task::spawn_blocking(move || match action {
                Action::Zoom { x1, y1, x2, y2 } => backend.screenshot_region(x1, y1, x2, y2),
                other => {
                    backend.execute(&other)?;
                    backend.screenshot()
                }
            })
            .await
            .map_err(exec_err)?
            .map_err(exec_err)?;

            Ok(ToolOutput::Image(ToolImage {
                media_type: img.media_type,
                data: img.data,
            }))
        })
    }
}
