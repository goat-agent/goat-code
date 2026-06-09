use std::{future::Future, pin::Pin};

use crate::{context::ToolContext, error::ToolError};

pub struct ToolImage {
    pub media_type: String,
    pub data: String,
}

pub enum ToolOutput {
    Text(String),
    Image(ToolImage),
}

impl ToolOutput {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            Self::Image(_) => None,
        }
    }
}

impl ToolOutput {
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }

    pub fn png(data: impl Into<String>) -> Self {
        Self::Image(ToolImage {
            media_type: "image/png".to_owned(),
            data: data.into(),
        })
    }
}

pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>>;

pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> serde_json::Value;
    fn run<'a>(&'a self, input: &'a str, ctx: &'a ToolContext) -> ToolFuture<'a>;
}
