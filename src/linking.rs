//! Insert Markdown links between existing notes.
//!
//! A link from note A to note B is a Markdown link appended to the end of A's
//! file, using a path relative to A's own directory.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Append a link to `target` (with visible text `title`) at the end of the
/// `source` file. The destination is `target` expressed relative to the source
/// file's directory, wrapped in `<>` if it contains spaces.
pub fn append_link(source: &Path, target: &Path, title: &str) -> Result<()> {
    let source_dir = source.parent().unwrap_or_else(|| Path::new(""));
    let rel = relative_path(source_dir, target);
    let link = markdown_link(title, &rel.to_string_lossy());

    let mut content =
        fs::read_to_string(source).with_context(|| format!("reading {}", source.display()))?;
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push('\n'); // blank line before the appended link
    content.push_str(&link);
    content.push('\n');
    fs::write(source, content).with_context(|| format!("writing {}", source.display()))?;
    Ok(())
}

/// Format `[title](dest)`, wrapping the destination in `<>` if it has spaces.
pub fn markdown_link(title: &str, dest: &str) -> String {
    if dest.contains(' ') {
        format!("[{title}](<{dest}>)")
    } else {
        format!("[{title}]({dest})")
    }
}

/// The path to `to`, relative to the directory `from_dir`. Both are expected to
/// be absolute (note paths and the vault root are canonicalized).
pub fn relative_path(from_dir: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from_dir.components().collect();
    let to_c: Vec<_> = to.components().collect();
    let common = from
        .iter()
        .zip(&to_c)
        .take_while(|(a, b)| a == b)
        .count();

    let mut result = PathBuf::new();
    for _ in common..from.len() {
        result.push("..");
    }
    for component in &to_c[common..] {
        result.push(component.as_os_str());
    }
    if result.as_os_str().is_empty() {
        // Same directory: just the filename.
        if let Some(name) = to.file_name() {
            result.push(name);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_same_dir() {
        let rel = relative_path(Path::new("/vault"), Path::new("/vault/note.md"));
        assert_eq!(rel, PathBuf::from("note.md"));
    }

    #[test]
    fn relative_into_subdir() {
        let rel = relative_path(Path::new("/vault"), Path::new("/vault/sub/note.md"));
        assert_eq!(rel, PathBuf::from("sub/note.md"));
    }

    #[test]
    fn relative_sibling_dir() {
        let rel = relative_path(Path::new("/vault/a"), Path::new("/vault/b/note.md"));
        assert_eq!(rel, PathBuf::from("../b/note.md"));
    }

    #[test]
    fn link_wraps_spaces() {
        assert_eq!(markdown_link("Note B", "a b.md"), "[Note B](<a b.md>)");
        assert_eq!(markdown_link("Note B", "b.md"), "[Note B](b.md)");
    }

    #[test]
    fn append_adds_link_at_end() {
        let dir = std::env::temp_dir().join(format!("constellate-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.md");
        let b = dir.join("b.md");
        fs::write(&a, "# A\nbody").unwrap();
        fs::write(&b, "# B\n").unwrap();

        append_link(&a, &b, "B").unwrap();
        let out = fs::read_to_string(&a).unwrap();
        assert_eq!(out, "# A\nbody\n\n[B](b.md)\n");

        let _ = fs::remove_dir_all(&dir);
    }
}
