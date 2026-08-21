//! Rename a Zettelkasten note's slug while keeping its `YYYYMMDDHHMM-` timestamp
//! prefix, rewriting inbound references across the vault so no links break.
//!
//! Only notes whose filename stem starts with a 12-digit timestamp followed by
//! `-` are renameable; free-form notes are left alone (issue #1, option b).

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::db::store::NoteRow;

/// The outcome of a rename: the new file path plus every file whose text was
/// changed (inbound references), for re-indexing.
pub struct RenameOutcome {
    pub old_path: String,
    pub new_path: String,
    /// Paths of source notes whose link references were rewritten.
    pub rewritten: Vec<String>,
}

/// Split a note's filename stem into its `YYYYMMDDHHMM` prefix and the trailing
/// slug, if it matches the Zettelkasten pattern `<12 digits>-<slug>`.
pub fn split_zettel(stem: &str) -> Option<(&str, &str)> {
    let (prefix, slug) = stem.split_once('-')?;
    if prefix.len() == 12 && prefix.bytes().all(|b| b.is_ascii_digit()) {
        Some((prefix, slug))
    } else {
        None
    }
}

/// Whether `path`'s filename is a renameable Zettelkasten note.
pub fn is_renameable(path: &str) -> bool {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(split_zettel)
        .is_some()
}

/// The current slug (the part after the timestamp prefix), if any.
pub fn current_slug(path: &str) -> Option<String> {
    let stem = Path::new(path).file_stem()?.to_str()?;
    split_zettel(stem).map(|(_, slug)| slug.to_string())
}

/// Lowercase, alphanumeric-only slug; runs of other characters collapse to a
/// single `-`, with no leading/trailing `-`. Mirrors `llm::slugify`.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Rename `old` — a Zettelkasten note — giving it `new_slug` (which is slugified)
/// while keeping its timestamp prefix and directory. Rewrites Markdown-link and
/// wikilink references to it in every other note, then moves the file on disk.
///
/// Nothing is written if the target already exists or the slug is empty/unchanged.
pub fn rename(old: &Path, new_slug: &str, notes: &[NoteRow]) -> Result<RenameOutcome> {
    let stem = old
        .file_stem()
        .and_then(|s| s.to_str())
        .context("note has no filename")?;
    let (prefix, old_slug) = split_zettel(stem).context("note is not a timestamped Zettel note")?;

    let slug = slugify(new_slug);
    if slug.is_empty() {
        bail!("new name is empty after slugifying");
    }
    if slug == old_slug {
        bail!("name unchanged");
    }

    let ext = old.extension().and_then(|e| e.to_str()).unwrap_or("md");
    let old_filename = old
        .file_name()
        .and_then(|s| s.to_str())
        .context("note has no filename")?
        .to_string();
    let new_filename = format!("{prefix}-{slug}.{ext}");
    let new_path = old.with_file_name(&new_filename);

    if new_path.exists() {
        bail!("a note named {new_filename} already exists");
    }

    let old_key = old.to_string_lossy().to_string();
    let new_key = new_path.to_string_lossy().to_string();

    // Rewrite references in every other note before moving the file.
    let mut rewritten = Vec::new();
    for note in notes {
        if note.path == old_key {
            continue;
        }
        let source = Path::new(&note.path);
        let Ok(text) = fs::read_to_string(source) else {
            continue;
        };
        let updated = rewrite_references(&text, stem, &old_filename, &slug, &new_filename);
        if updated != text {
            fs::write(source, updated)
                .with_context(|| format!("writing {}", source.display()))?;
            rewritten.push(note.path.clone());
        }
    }

    fs::rename(old, &new_path)
        .with_context(|| format!("renaming {} to {}", old.display(), new_path.display()))?;

    Ok(RenameOutcome {
        old_path: old_key,
        new_path: new_key,
        rewritten,
    })
}

/// Rewrite every reference to the renamed note within one note's text.
///
/// Handles two link forms: Markdown-link destinations `](path)` / `](<path>)`
/// whose final path component names the old file, and `[[wikilinks]]` whose
/// target stem matches. Only the filename component changes — the directory
/// prefix (and thus the relative path) is preserved.
fn rewrite_references(
    text: &str,
    old_stem: &str,
    old_filename: &str,
    new_slug: &str,
    new_filename: &str,
) -> String {
    let with_md = rewrite_markdown_links(text, old_filename, new_filename);
    rewrite_wikilinks(&with_md, old_stem, new_slug)
}

/// Swap the filename component of any Markdown-link destination that points at
/// `old_filename` for `new_filename`, re-wrapping in `<>` when needed.
fn rewrite_markdown_links(text: &str, old_filename: &str, new_filename: &str) -> String {
    // Build into a byte buffer: we only ever emit ASCII or copy original bytes,
    // so the result stays valid UTF-8 (byte-to-char casting would corrupt it).
    let mut out: Vec<u8> = Vec::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // A destination opens with "](" following a link label.
        if bytes[i] == b'(' && i > 0 && bytes[i - 1] == b']' {
            if let Some(close) = find_dest_close(text, i + 1) {
                let raw = &text[i + 1..close]; // between the parens
                let inner = raw.strip_prefix('<').and_then(|s| s.strip_suffix('>'));
                let dest = inner.unwrap_or(raw);
                if let Some(rebuilt) = swap_dest_filename(dest, old_filename, new_filename) {
                    let wrapped = if rebuilt.contains(' ') {
                        format!("<{rebuilt}>")
                    } else {
                        rebuilt
                    };
                    out.push(b'(');
                    out.extend_from_slice(wrapped.as_bytes());
                    out.push(b')');
                    i = close + 1;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).expect("rewrite preserves UTF-8")
}

/// Find the byte index of the `)` closing a destination that starts at `start`.
/// Handles an optional `<...>` wrapper (which may itself contain `)`).
fn find_dest_close(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(start) == Some(&b'<') {
        let gt = text[start..].find('>')? + start;
        if bytes.get(gt + 1) == Some(&b')') {
            return Some(gt + 1);
        }
        return None;
    }
    // Bare destination: runs until the next ')' (destinations can't contain one).
    text[start..].find(')').map(|p| p + start)
}

/// If `dest`'s final path component names `old_filename`, return `dest` with that
/// component replaced by `new_filename`; otherwise `None`.
fn swap_dest_filename(dest: &str, old_filename: &str, new_filename: &str) -> Option<String> {
    let (dir, name) = match dest.rsplit_once('/') {
        Some((dir, name)) => (Some(dir), name),
        None => (None, dest),
    };
    if !name.eq_ignore_ascii_case(old_filename) {
        return None;
    }
    Some(match dir {
        Some(dir) => format!("{dir}/{new_filename}"),
        None => new_filename.to_string(),
    })
}

/// Rewrite `[[old_stem]]` / `[[old_stem|alias]]` targets to the new slug,
/// preserving any alias. Matching is case-insensitive on the whole target.
fn rewrite_wikilinks(text: &str, old_stem: &str, new_slug: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("[[") {
        let after_open = &rest[open + 2..];
        if let Some(close) = after_open.find("]]") {
            out.push_str(&rest[..open + 2]);
            let inner = &after_open[..close];
            let (target, alias) = match inner.split_once('|') {
                Some((t, a)) => (t, Some(a)),
                None => (inner, None),
            };
            if target.trim().eq_ignore_ascii_case(old_stem) {
                out.push_str(new_slug);
                if let Some(alias) = alias {
                    out.push('|');
                    out.push_str(alias);
                }
            } else {
                out.push_str(inner);
            }
            out.push_str("]]");
            rest = &after_open[close + 2..];
        } else {
            break;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_recognizes_zettel() {
        assert_eq!(split_zettel("202408211230-old-slug"), Some(("202408211230", "old-slug")));
        assert_eq!(split_zettel("notes-about-x"), None);
        assert_eq!(split_zettel("20240821-short"), None); // 8 digits, not 12
    }

    #[test]
    fn renameable_gate() {
        assert!(is_renameable("/v/202408211230-topic.md"));
        assert!(!is_renameable("/v/ideas.md"));
    }

    #[test]
    fn slugify_matches_llm_style() {
        assert_eq!(slugify("My New Title!"), "my-new-title");
        assert_eq!(slugify("  spaced / out  "), "spaced-out");
    }

    #[test]
    fn markdown_link_same_dir() {
        let out = rewrite_markdown_links(
            "see [Topic](202408211230-old.md) here",
            "202408211230-old.md",
            "202408211230-new.md",
        );
        assert_eq!(out, "see [Topic](202408211230-new.md) here");
    }

    #[test]
    fn markdown_link_preserves_dir_and_wraps_spaces() {
        let out = rewrite_markdown_links(
            "[T](../sub/202408211230-old.md)",
            "202408211230-old.md",
            "202408211230-new name.md",
        );
        assert_eq!(out, "[T](<../sub/202408211230-new name.md>)");
    }

    #[test]
    fn markdown_link_unwraps_angle_source() {
        let out = rewrite_markdown_links(
            "[T](<a b/202408211230-old.md>)",
            "202408211230-old.md",
            "202408211230-new.md",
        );
        assert_eq!(out, "[T](<a b/202408211230-new.md>)");
    }

    #[test]
    fn leaves_unrelated_links_alone() {
        let text = "[Other](202408211230-different.md) and (plain parens)";
        assert_eq!(
            rewrite_markdown_links(text, "202408211230-old.md", "202408211230-new.md"),
            text
        );
    }

    #[test]
    fn wikilink_with_alias() {
        let out = rewrite_wikilinks(
            "ref [[202408211230-old|My Note]] end",
            "202408211230-old",
            "202408211230-new",
        );
        assert_eq!(out, "ref [[202408211230-new|My Note]] end");
    }

    #[test]
    fn wikilink_by_title_untouched() {
        let text = "[[Some Title]]";
        assert_eq!(rewrite_wikilinks(text, "202408211230-old", "202408211230-new"), text);
    }

    #[test]
    fn rename_moves_file_and_rewrites_inbound_link() {
        let dir =
            std::env::temp_dir().join(format!("constellate-rename-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let old = dir.join("202408211230-old-slug.md");
        let src = dir.join("202408211231-other.md");
        fs::write(&old, "# Old\n").unwrap();
        fs::write(&src, "See [Old](202408211230-old-slug.md)\n").unwrap();

        let notes = vec![
            NoteRow {
                path: old.to_string_lossy().to_string(),
                title: "Old".into(),
                content: String::new(),
                tags: vec![],
                links: vec![],
            },
            NoteRow {
                path: src.to_string_lossy().to_string(),
                title: "Other".into(),
                content: String::new(),
                tags: vec![],
                links: vec!["202408211230-old-slug".into()],
            },
        ];

        let outcome = rename(&old, "New Title", &notes).unwrap();
        let new = dir.join("202408211230-new-title.md");

        assert!(!old.exists());
        assert!(new.exists());
        assert_eq!(outcome.new_path, new.to_string_lossy());
        assert_eq!(outcome.rewritten, vec![src.to_string_lossy().to_string()]);
        assert_eq!(
            fs::read_to_string(&src).unwrap(),
            "See [Old](202408211230-new-title.md)\n"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_refuses_existing_target() {
        let dir =
            std::env::temp_dir().join(format!("constellate-rename-clash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let old = dir.join("202408211230-old.md");
        let clash = dir.join("202408211230-taken.md");
        fs::write(&old, "x").unwrap();
        fs::write(&clash, "y").unwrap();

        let notes = vec![NoteRow {
            path: old.to_string_lossy().to_string(),
            title: "Old".into(),
            content: String::new(),
            tags: vec![],
            links: vec![],
        }];

        assert!(rename(&old, "taken", &notes).is_err());
        assert!(old.exists(), "old file must remain on a refused rename");

        let _ = fs::remove_dir_all(&dir);
    }
}
