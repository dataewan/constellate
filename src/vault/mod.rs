pub mod chunker;
pub mod scanner;

use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::db::store::Store;

/// A single heading-grouped section of a note.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Heading text for the section (empty for content before the first heading).
    pub header: String,
    /// Plain-text content of the section.
    pub content: String,
}

/// A parsed note ready to be written to the index.
#[derive(Debug, Clone)]
pub struct ParsedNote {
    /// Absolute path to the source file, used as the stable key.
    pub path: PathBuf,
    /// Display title (frontmatter `title:`, first heading, or filename stem).
    pub title: String,
    /// Raw YAML frontmatter block, if present.
    pub frontmatter: Option<String>,
    /// Full source text of the note.
    pub content: String,
    /// Outgoing references: `[[wikilinks]]` and Markdown link destinations.
    pub links: Vec<String>,
    /// Tags from frontmatter and inline `#tags`.
    pub tags: Vec<String>,
    /// Heading-grouped chunks (used by later embedding phases).
    pub chunks: Vec<Chunk>,
}

/// Summary of an index synchronization pass.
#[derive(Debug, Default)]
pub struct SyncStats {
    pub parsed: usize,
    pub skipped: usize,
    pub deleted: usize,
    pub errors: usize,
}

/// Deterministic content hash used for incremental change detection.
pub fn hash_content(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// Full index synchronization: parse new/changed files, skip unchanged ones,
/// and remove notes whose source files have disappeared.
pub fn sync_all(store: &mut Store, vault: &Path) -> Result<SyncStats> {
    let mut stats = SyncStats::default();
    let files = scanner::discover(vault);
    let mut present: HashSet<String> = HashSet::with_capacity(files.len());

    for path in &files {
        present.insert(path_key(path));
        match sync_one(store, path) {
            Ok(true) => stats.parsed += 1,
            Ok(false) => stats.skipped += 1,
            Err(_) => stats.errors += 1,
        }
    }

    for existing in store.all_paths()? {
        if !present.contains(&existing) {
            store.delete_note(&existing)?;
            stats.deleted += 1;
        }
    }

    Ok(stats)
}

/// Re-index a specific set of paths (from the file watcher or an editor session).
/// Missing files are deleted from the index. Returns true if anything changed.
pub fn sync_paths(store: &mut Store, vault: &Path, paths: &[PathBuf]) -> Result<bool> {
    let mut changed = false;
    for path in paths {
        if !path.starts_with(vault) || !scanner::is_markdown(path) {
            continue;
        }
        if path.exists() {
            if sync_one(store, path)? {
                changed = true;
            }
        } else {
            store.delete_note(&path_key(path))?;
            changed = true;
        }
    }
    Ok(changed)
}

/// Index a single file, skipping it when its content hash is unchanged.
/// Returns true if the note was (re)written.
fn sync_one(store: &mut Store, path: &Path) -> Result<bool> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let hash = hash_content(&content);

    if store.stored_hash(&path_key(path))?.as_deref() == Some(hash.as_str()) {
        return Ok(false);
    }

    let note = parse(path, content);
    store.upsert_note(&note, &hash)?;
    Ok(true)
}

/// Parse raw note text into a [`ParsedNote`].
pub fn parse(path: &Path, content: String) -> ParsedNote {
    let (frontmatter, body) = split_frontmatter(&content);
    let chunks = chunker::chunk(body);
    let mut links = chunker::markdown_links(body);
    links.extend(extract_wikilinks(body));
    dedup(&mut links);

    let mut tags = frontmatter
        .as_deref()
        .map(frontmatter_tags)
        .unwrap_or_default();
    tags.extend(extract_inline_tags(body));
    dedup(&mut tags);

    let title = derive_title(path, frontmatter.as_deref(), body);

    ParsedNote {
        path: path.to_path_buf(),
        title,
        frontmatter,
        content,
        links,
        tags,
        chunks,
    }
}

/// Split a leading `---` YAML frontmatter block from the body.
fn split_frontmatter(content: &str) -> (Option<String>, &str) {
    let rest = match content.strip_prefix("---\n") {
        Some(r) => r,
        None => return (None, content),
    };
    // Find the closing delimiter line.
    if let Some(end) = rest.find("\n---\n") {
        let fm = &rest[..end];
        let body = &rest[end + 5..];
        (Some(fm.to_string()), body)
    } else if let Some(end) = rest.find("\n---") {
        // Closing delimiter at end of file.
        let fm = &rest[..end];
        (Some(fm.to_string()), "")
    } else {
        (None, content)
    }
}

fn derive_title(path: &Path, frontmatter: Option<&str>, body: &str) -> String {
    if let Some(fm) = frontmatter {
        if let Some(t) = frontmatter_title(fm) {
            return t;
        }
    }
    if let Some(h) = first_heading(body) {
        return h;
    }
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn frontmatter_title(fm: &str) -> Option<String> {
    for line in fm.lines() {
        if let Some(rest) = line.trim().strip_prefix("title:") {
            let t = rest.trim().trim_matches(['"', '\'']).trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn frontmatter_tags(fm: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for line in fm.lines() {
        if let Some(rest) = line.trim().strip_prefix("tags:") {
            let rest = rest.trim();
            // Inline list form: tags: [a, b, c]
            let inner = rest.trim_start_matches('[').trim_end_matches(']');
            for t in inner.split([',', ' ']) {
                let t = t.trim().trim_matches(['"', '\'', '-']).trim();
                if !t.is_empty() {
                    tags.push(t.to_string());
                }
            }
        }
    }
    tags
}

fn first_heading(body: &str) -> Option<String> {
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let heading = rest.trim_start_matches('#').trim();
            if !heading.is_empty() {
                return Some(heading.to_string());
            }
        }
    }
    None
}

/// Extract `[[wikilink]]` targets (the part before any `|` alias).
fn extract_wikilinks(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(close) = body[i + 2..].find("]]") {
                let inner = &body[i + 2..i + 2 + close];
                let target = inner.split('|').next().unwrap_or(inner).trim();
                if !target.is_empty() {
                    out.push(target.to_string());
                }
                i = i + 2 + close + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Extract inline `#tags` (whitespace-preceded, not Markdown headings).
fn extract_inline_tags(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        // Skip Markdown ATX headings ("# ", "## ", ...).
        if line.trim_start().starts_with("# ")
            || line.trim_start().starts_with("## ")
            || line.trim_start().starts_with("### ")
        {
            continue;
        }
        for (idx, ch) in line.char_indices() {
            if ch != '#' {
                continue;
            }
            // Must be at start of line or preceded by whitespace.
            let preceded_ok = idx == 0
                || line[..idx]
                    .chars()
                    .next_back()
                    .map(|c| c.is_whitespace())
                    .unwrap_or(true);
            if !preceded_ok {
                continue;
            }
            let tag: String = line[idx + 1..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '/')
                .collect();
            if !tag.is_empty() && tag.chars().any(|c| c.is_alphabetic()) {
                out.push(tag);
            }
        }
    }
    out
}

fn dedup(items: &mut Vec<String>) {
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(item.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parse_str(name: &str, content: &str) -> ParsedNote {
        parse(&PathBuf::from(name), content.to_string())
    }

    #[test]
    fn title_from_frontmatter_wins() {
        let note = parse_str(
            "note.md",
            "---\ntitle: My Title\ntags: [a, b]\n---\n# Heading\nBody",
        );
        assert_eq!(note.title, "My Title");
        assert!(note.frontmatter.is_some());
    }

    #[test]
    fn title_falls_back_to_first_heading_then_filename() {
        let with_heading = parse_str("x.md", "# The Heading\ntext");
        assert_eq!(with_heading.title, "The Heading");

        let bare = parse_str("my-note.md", "just text, no heading");
        assert_eq!(bare.title, "my-note");
    }

    #[test]
    fn extracts_wikilinks_with_aliases() {
        let note = parse_str("n.md", "See [[Other Note]] and [[Target|alias]].");
        assert!(note.links.contains(&"Other Note".to_string()));
        assert!(note.links.contains(&"Target".to_string()));
    }

    #[test]
    fn extracts_frontmatter_and_inline_tags() {
        let note = parse_str(
            "n.md",
            "---\ntags: [rust, cli]\n---\nText with #project and #rust tags.",
        );
        assert!(note.tags.contains(&"rust".to_string()));
        assert!(note.tags.contains(&"cli".to_string()));
        assert!(note.tags.contains(&"project".to_string()));
        // Deduplicated across frontmatter + inline.
        assert_eq!(note.tags.iter().filter(|t| *t == "rust").count(), 1);
    }

    #[test]
    fn heading_lines_are_not_inline_tags() {
        let note = parse_str("n.md", "# Not A Tag\nbody");
        assert!(note.tags.is_empty());
    }

    #[test]
    fn chunker_splits_by_heading() {
        let chunks = chunker::chunk("intro text\n\n# First\nalpha\n\n## Second\nbeta");
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].header, "");
        assert_eq!(chunks[1].header, "First");
        assert!(chunks[1].content.contains("alpha"));
        assert_eq!(chunks[2].header, "Second");
    }

    #[test]
    fn hash_is_stable_and_content_sensitive() {
        assert_eq!(hash_content("abc"), hash_content("abc"));
        assert_ne!(hash_content("abc"), hash_content("abd"));
    }
}
