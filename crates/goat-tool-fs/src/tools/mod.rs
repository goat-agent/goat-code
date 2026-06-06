pub mod edit;
pub mod read;
pub mod write;

use std::path::Path;

pub(crate) fn relative_display(cwd: &Path, resolved: &Path) -> String {
    resolved
        .strip_prefix(cwd)
        .unwrap_or(resolved)
        .display()
        .to_string()
}
