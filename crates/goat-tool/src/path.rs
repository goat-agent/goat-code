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
    resolve_with_extra(cwd, None, raw)
}

pub fn resolve_with_extra(
    cwd: &Path,
    extra: Option<&Path>,
    raw: &str,
) -> Result<PathBuf, ToolError> {
    resolve_with_policy(cwd, extra, &[], raw)
}

pub fn resolve_with_policy(
    cwd: &Path,
    extra: Option<&Path>,
    blocked: &[PathBuf],
    raw: &str,
) -> Result<PathBuf, ToolError> {
    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    };
    let normalized = lexical_normalize(&joined);
    if !within(cwd, extra, &normalized) {
        return Err(ToolError::PathEscape {
            path: raw.to_owned(),
        });
    }
    if blocked_path(blocked, &normalized) {
        return Err(ToolError::PathBlocked {
            path: raw.to_owned(),
        });
    }
    if normalized.exists() {
        let canonical = normalized.canonicalize().map_err(|source| ToolError::Io {
            path: raw.to_owned(),
            source,
        })?;
        if !within(cwd, extra, &canonical) {
            return Err(ToolError::PathEscape {
                path: raw.to_owned(),
            });
        }
        if blocked_path(blocked, &canonical) {
            return Err(ToolError::PathBlocked {
                path: raw.to_owned(),
            });
        }
    }
    Ok(normalized)
}

pub fn blocked_path(blocked: &[PathBuf], path: &Path) -> bool {
    blocked.iter().any(|root| path.starts_with(root))
}

fn within(cwd: &Path, extra: Option<&Path>, path: &Path) -> bool {
    path.starts_with(cwd) || extra.is_some_and(|allowed| path == allowed)
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
    use std::path::PathBuf;

    use super::{resolve_in_cwd, resolve_with_policy};
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

    #[test]
    fn blocked_path_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let blocked = vec![cwd.join(".goat").join("worktrees")];
        let result = resolve_with_policy(&cwd, None, &blocked, ".goat/worktrees/plan/file");
        assert!(matches!(result, Err(ToolError::PathBlocked { .. })));
    }

    #[test]
    fn similarly_named_path_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let blocked = vec![cwd.join(".goat").join("worktrees")];
        let resolved = resolve_with_policy(&cwd, None, &blocked, ".goat/worktrees2/file").unwrap();
        assert_eq!(resolved, cwd.join(".goat").join("worktrees2").join("file"));
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

    #[test]
    fn blocked_absolute_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let blocked = vec![cwd.join(".goat").join("worktrees")];
        let raw = cwd
            .join(".goat")
            .join("worktrees")
            .join("plan")
            .display()
            .to_string();
        let result = resolve_with_policy(&cwd, None, &blocked, &raw);
        assert!(matches!(result, Err(ToolError::PathBlocked { .. })));
    }

    #[test]
    fn extra_path_does_not_bypass_block() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let blocked = vec![cwd.join(".goat").join("worktrees")];
        let target = cwd.join(".goat").join("worktrees").join("plan");
        let result = resolve_with_policy(
            &cwd,
            Some(PathBuf::from(&target).as_path()),
            &blocked,
            target.to_str().unwrap(),
        );
        assert!(matches!(result, Err(ToolError::PathBlocked { .. })));
    }
}
