mod glob;
mod grep;
mod web_search;

pub use glob::GlobTool;
pub use grep::GrepTool;
pub use web_search::WebSearchTool;

pub fn all() -> Vec<Box<dyn goat_tool::Tool>> {
    vec![
        Box::new(GrepTool),
        Box::new(GlobTool),
        Box::new(WebSearchTool::new()),
    ]
}

pub(crate) fn ignore_error(err: &ignore::Error) -> goat_tool::ToolError {
    goat_tool::ToolError::Io {
        path: "<glob/walk>".to_owned(),
        source: std::io::Error::other(err.to_string()),
    }
}
