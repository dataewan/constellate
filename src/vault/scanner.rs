use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::config::STATE_DIR;

/// Recursively discover Markdown files in the vault, skipping the app's own
/// state directory and hidden directories.
pub fn discover(vault: &Path) -> Vec<PathBuf> {
    WalkDir::new(vault)
        .into_iter()
        .filter_entry(|entry| !is_excluded_dir(entry.path(), entry.depth()))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|path| is_markdown(path))
        .collect()
}

/// True for the `.constellate` state directory and any hidden directory.
fn is_excluded_dir(path: &Path, depth: usize) -> bool {
    if depth == 0 {
        return false; // never exclude the vault root itself
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name == STATE_DIR || (name.starts_with('.') && path.is_dir())
}

/// True if the path has a Markdown extension.
pub fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("md" | "markdown" | "mdown" | "mkd")
    )
}
