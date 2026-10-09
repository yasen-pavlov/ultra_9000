//! project_root(cwd, roots): nearest git directory/worktree marker; filtering is external.
use std::path::{Path, PathBuf};
pub fn project_root(cwd: &Path, _roots: &[PathBuf]) -> PathBuf {
    let normalized = cwd.canonicalize().unwrap_or_else(|_| cwd.to_owned());
    for ancestor in normalized.ancestors() {
        if ancestor.join(".git").is_dir() || ancestor.join(".git").is_file() {
            return ancestor.to_owned();
        }
    }
    normalized
}
#[cfg(test)]
mod tests;
