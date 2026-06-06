use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::error::ToolError;

pub struct ToolContext {
    pub cwd: PathBuf,
    pub bash_timeout: Duration,
    pub max_output_bytes: usize,
}

impl ToolContext {
    pub fn new(cwd: &Path) -> Result<Self, ToolError> {
        let cwd = cwd
            .canonicalize()
            .map_err(|source| ToolError::Cwd { source })?;
        Ok(Self {
            cwd,
            bash_timeout: Duration::from_mins(2),
            max_output_bytes: 64 * 1024,
        })
    }
}
