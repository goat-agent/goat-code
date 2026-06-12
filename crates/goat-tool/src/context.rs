use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{error::ToolError, path::resolve_with_extra, policy::SandboxPolicy};

pub struct ToolContext {
    pub cwd: PathBuf,
    pub bash_timeout: Duration,
    pub max_output_bytes: usize,
    pub extra_path: Option<PathBuf>,
    pub write_allow: Option<PathBuf>,
    pub exec_policy: SandboxPolicy,
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
            extra_path: None,
            write_allow: None,
            exec_policy: SandboxPolicy::Full,
        })
    }

    pub fn resolve(&self, raw: &str) -> Result<PathBuf, ToolError> {
        resolve_with_extra(&self.cwd, self.extra_path.as_deref(), raw)
    }

    pub fn ensure_writable(&self, resolved: &Path, raw: &str) -> Result<(), ToolError> {
        match &self.write_allow {
            Some(allowed) if resolved != allowed.as_path() => Err(ToolError::WriteBlocked {
                path: raw.to_owned(),
            }),
            _ => Ok(()),
        }
    }
}
