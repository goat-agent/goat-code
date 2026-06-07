use std::path::{Component, Path, PathBuf};

use crate::error::ToolError;

pub fn relative_display(cwd: &Path, resolved: &Path) -> String {
    resolved
        .strip_prefix(cwd)
        .unwrap_or(resolved)
        .display()
        .to_string()
}

pub fn resolve_in_cwd(cwd: &Path, raw: &str) -> Result<PathBuf, ToolError> {
    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    };
    let normalized = lexical_normalize(&joined);
    if !normalized.starts_with(cwd) {
        return Err(ToolError::PathEscape {
            path: raw.to_owned(),
        });
    }
    if normalized.exists() {
        let canonical = normalized.canonicalize().map_err(|source| ToolError::Io {
            path: raw.to_owned(),
            source,
        })?;
        if !canonical.starts_with(cwd) {
            return Err(ToolError::PathEscape {
                path: raw.to_owned(),
            });
        }
    }
    Ok(normalized)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::resolve_in_cwd;
    use crate::error::ToolError;

    #[test]
    fn parent_traversal_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let result = resolve_in_cwd(&cwd, "../escape");
        assert!(matches!(result, Err(ToolError::PathEscape { .. })));
    }

    #[test]
    fn absolute_outside_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let result = resolve_in_cwd(&cwd, "/etc/passwd");
        assert!(matches!(result, Err(ToolError::PathEscape { .. })));
    }

    #[test]
    fn relative_within_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let resolved = resolve_in_cwd(&cwd, "file.txt").unwrap();
        assert_eq!(resolved, cwd.join("file.txt"));
    }

    #[test]
    fn inner_traversal_stays_within() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let resolved = resolve_in_cwd(&cwd, "./subdir/../file").unwrap();
        assert_eq!(resolved, cwd.join("file"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_rejected() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        symlink("/etc", cwd.join("link")).unwrap();
        let result = resolve_in_cwd(&cwd, "link/passwd");
        assert!(matches!(result, Err(ToolError::PathEscape { .. })));
    }
}
