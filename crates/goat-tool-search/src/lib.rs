mod glob;
mod grep;

pub use glob::GlobTool;
pub use grep::GrepTool;

pub fn all() -> Vec<Box<dyn goat_tool::Tool>> {
    vec![Box::new(GrepTool), Box::new(GlobTool)]
}

pub(crate) fn ignore_error(err: &ignore::Error) -> goat_tool::ToolError {
    goat_tool::ToolError::Io {
        path: String::new(),
        source: std::io::Error::other(err.to_string()),
    }
}
