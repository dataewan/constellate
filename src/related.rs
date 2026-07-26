//! Cheap, deterministic relatedness: link graph + tag overlap + keyword overlap.
//! No embeddings — semantic similarity is layered on in a later phase.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::db::store::NoteRow;

/// Scoring weights for each relatedness signal.
const W_LINK: f32 = 3.0; // this note links to the candidate
const W_BACKLINK: f32 = 3.0; // the candidate links to this note
const W_SHARED_TAG: f32 = 1.5; // per shared tag
const W_CO_LINK: f32 = 1.0; // per shared outgoing target
const W_KEYWORD: f32 = 0.5; // per shared title keyword

/// A related note with its combined score and the signals that produced it.
#[derive(Debug, Clone)]
pub struct RelatedNote {
    pub path: String,
    pub title: String,
    pub score: f32,
    pub reason: String,
}

/// Merge several ranked lists (e.g. cheap signals + semantic similarity) into
/// one, summing scores and joining reasons for notes that appear in more than
/// one list. Returns the top `limit` by score.
pub fn merge(lists: impl IntoIterator<Item = Vec<RelatedNote>>, limit: usize) -> Vec<RelatedNote> {
    let mut by_path: HashMap<String, RelatedNote> = HashMap::new();
    for list in lists {
        for note in list {
            by_path
                .entry(note.path.clone())
                .and_modify(|existing| {
                    existing.score += note.score;
                    if !existing.reason.is_empty() && !note.reason.is_empty() {
                        existing.reason.push_str(" · ");
                    }
                    existing.reason.push_str(&note.reason);
                })
                .or_insert(note);
        }
    }

    let mut merged: Vec<RelatedNote> = by_path.into_values().collect();
    merged.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    merged.truncate(limit);
    merged
}

struct Entry {
    path: String,
    title: String,
    tags: HashSet<String>,
    out_paths: HashSet<String>,
    keywords: HashSet<String>,
}

/// Precomputed relatedness index over the whole vault. Rebuilt on re-index.
pub struct RelatedIndex {
    entries: Vec<Entry>,
    by_path: HashMap<String, usize>,
}

impl RelatedIndex {
    pub fn build(notes: &[NoteRow]) -> Self {
        // First pass: resolve titles and filename stems to note paths.
        let mut resolve: HashMap<String, String> = HashMap::new();
        for note in notes {
            resolve
                .entry(note.title.to_lowercase())
                .or_insert_with(|| note.path.clone());
            if let Some(stem) = Path::new(&note.path).file_stem().and_then(|s| s.to_str()) {
                resolve
                    .entry(stem.to_lowercase())
                    .or_insert_with(|| note.path.clone());
            }
        }

        // Second pass: build entries with resolved outgoing links.
        let mut entries = Vec::with_capacity(notes.len());
        let mut by_path = HashMap::with_capacity(notes.len());
        for (i, note) in notes.iter().enumerate() {
            by_path.insert(note.path.clone(), i);

            let mut out_paths = HashSet::new();
            for link in &note.links {
                if let Some(target) = resolve.get(&normalize_target(link)) {
                    if *target != note.path {
                        out_paths.insert(target.clone());
                    }
                }
            }

            entries.push(Entry {
                path: note.path.clone(),
                title: note.title.clone(),
                tags: note.tags.iter().map(|t| t.to_lowercase()).collect(),
                out_paths,
                keywords: tokenize(&note.title).collect(),
            });
        }

        RelatedIndex { entries, by_path }
    }

    /// Whether notes `a` and `b` are directly linked in either direction.
    pub fn are_linked(&self, a: &str, b: &str) -> bool {
        let links_to = |from: &str, to: &str| {
            self.by_path
                .get(from)
                .map(|&i| self.entries[i].out_paths.contains(to))
                .unwrap_or(false)
        };
        links_to(a, b) || links_to(b, a)
    }

    /// Rank notes related to `path`, most relevant first, capped at `limit`.
    pub fn related(&self, path: &str, limit: usize) -> Vec<RelatedNote> {
        let Some(&i) = self.by_path.get(path) else {
            return Vec::new();
        };
        let me = &self.entries[i];

        let mut out: Vec<RelatedNote> = Vec::new();
        for (j, other) in self.entries.iter().enumerate() {
            if j == i {
                continue;
            }

            let mut score = 0.0;
            let mut reasons: Vec<String> = Vec::new();

            if me.out_paths.contains(&other.path) {
                score += W_LINK;
                reasons.push("link".to_string());
            }
            if other.out_paths.contains(&me.path) {
                score += W_BACKLINK;
                reasons.push("backlink".to_string());
            }

            let shared_tags = me.tags.intersection(&other.tags).count();
            if shared_tags > 0 {
                score += W_SHARED_TAG * shared_tags as f32;
                reasons.push(format!("{shared_tags} tag{}", plural(shared_tags)));
            }

            let co_links = me.out_paths.intersection(&other.out_paths).count();
            if co_links > 0 {
                score += W_CO_LINK * co_links as f32;
                reasons.push("co-links".to_string());
            }

            let shared_kw = me.keywords.intersection(&other.keywords).count();
            if shared_kw > 0 {
                score += W_KEYWORD * shared_kw as f32;
                reasons.push("keywords".to_string());
            }

            if score > 0.0 {
                out.push(RelatedNote {
                    path: other.path.clone(),
                    title: other.title.clone(),
                    score,
                    // Show the two strongest-listed signals to keep it compact.
                    reason: reasons.into_iter().take(2).collect::<Vec<_>>().join(" · "),
                });
            }
        }

        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        out.truncate(limit);
        out
    }
}

/// Normalize a link target to match the resolver keys: drop any path prefix,
/// strip a Markdown extension, take the alias-free part, and lowercase.
fn normalize_target(target: &str) -> String {
    let target = target.split('|').next().unwrap_or(target).trim();
    let last = target.rsplit(['/', '\\']).next().unwrap_or(target);
    let stem = last
        .strip_suffix(".md")
        .or_else(|| last.strip_suffix(".markdown"))
        .unwrap_or(last);
    stem.to_lowercase()
}

fn tokenize(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_lowercase())
        .filter(|t| t.len() >= 3 && !STOPWORDS.contains(&t.as_str()))
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "this", "that", "into", "your", "you", "are", "was",
    "were", "have", "has", "not", "but", "all", "any", "how", "why", "what", "when", "which",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn note(path: &str, title: &str, tags: &[&str], links: &[&str]) -> NoteRow {
        NoteRow {
            path: path.to_string(),
            title: title.to_string(),
            content: String::new(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            links: links.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn direct_link_and_backlink_rank_together() {
        let notes = vec![
            note("/v/a.md", "Alpha", &[], &["Beta"]),
            note("/v/b.md", "Beta", &[], &["Alpha"]),
            note("/v/c.md", "Gamma", &[], &[]),
        ];
        let idx = RelatedIndex::build(&notes);
        let related = idx.related("/v/a.md", 10);
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].title, "Beta");
        // Both an outgoing link and a backlink contribute.
        assert!(related[0].score >= W_LINK + W_BACKLINK);
    }

    #[test]
    fn shared_tags_create_relatedness() {
        let notes = vec![
            note("/v/a.md", "Alpha", &["rust", "cli"], &[]),
            note("/v/b.md", "Beta", &["rust"], &[]),
            note("/v/c.md", "Gamma", &["python"], &[]),
        ];
        let idx = RelatedIndex::build(&notes);
        let related = idx.related("/v/a.md", 10);
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].title, "Beta");
        assert!(related[0].reason.contains("tag"));
    }

    #[test]
    fn wikilink_resolves_by_filename_stem() {
        let notes = vec![
            note("/v/a.md", "Alpha", &[], &["b"]),
            note("/v/b.md", "Beta", &[], &[]),
        ];
        let idx = RelatedIndex::build(&notes);
        let related = idx.related("/v/a.md", 10);
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].reason, "link");
    }

    #[test]
    fn unrelated_notes_are_absent() {
        let notes = vec![
            note("/v/a.md", "Alpha", &["x"], &[]),
            note("/v/b.md", "Beta", &["y"], &[]),
        ];
        let idx = RelatedIndex::build(&notes);
        assert!(idx.related("/v/a.md", 10).is_empty());
    }

    #[test]
    fn merge_sums_scores_and_joins_reasons() {
        let cheap = vec![RelatedNote {
            path: "/v/b.md".into(),
            title: "Beta".into(),
            score: 3.0,
            reason: "link".into(),
        }];
        let semantic = vec![
            RelatedNote {
                path: "/v/b.md".into(),
                title: "Beta".into(),
                score: 2.0,
                reason: "similar 80%".into(),
            },
            RelatedNote {
                path: "/v/c.md".into(),
                title: "Gamma".into(),
                score: 1.0,
                reason: "similar 60%".into(),
            },
        ];
        let merged = merge([cheap, semantic], 10);
        assert_eq!(merged.len(), 2);
        // Beta appears in both lists: scores sum, reasons join.
        assert_eq!(merged[0].title, "Beta");
        assert_eq!(merged[0].score, 5.0);
        assert!(merged[0].reason.contains("link"));
        assert!(merged[0].reason.contains("similar 80%"));
    }
}
