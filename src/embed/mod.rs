pub mod ollama;
#[cfg(feature = "fastembed")]
pub mod fastembed;

use std::collections::HashMap;

use crate::db::store::NoteRow;
use crate::related::RelatedNote;

/// Selects and constructs an embedding backend. The `FastEmbed` variant only
/// exists when built with the `fastembed` feature.
#[derive(Debug, Clone)]
pub enum Backend {
    /// Local Ollama server (default).
    Ollama { url: String, model: String },
    /// In-process ONNX embeddings via the `fastembed` crate.
    #[cfg(feature = "fastembed")]
    FastEmbed,
}

impl Backend {
    /// Identifier stored in `meta.embed_model` so switching backend/model
    /// discards incompatible cached vectors.
    pub fn model_id(&self) -> String {
        match self {
            Backend::Ollama { model, .. } => model.clone(),
            #[cfg(feature = "fastembed")]
            Backend::FastEmbed => format!("fastembed:{}", fastembed::MODEL_ID),
        }
    }

    /// Human-readable description for the startup line.
    pub fn describe(&self) -> String {
        match self {
            Backend::Ollama { url, model } => format!("{model} via Ollama at {url}"),
            #[cfg(feature = "fastembed")]
            Backend::FastEmbed => {
                format!("{} via fastembed (in-process ONNX)", fastembed::MODEL_ID)
            }
        }
    }

    /// Construct the embedder. Runs on the worker thread; may download a model
    /// or initialize a runtime, so it can fail.
    pub fn build(&self) -> Result<Box<dyn Embedder>, EmbedError> {
        match self {
            Backend::Ollama { url, model } => {
                Ok(Box::new(ollama::OllamaEmbedder::new(url.clone(), model.clone())))
            }
            #[cfg(feature = "fastembed")]
            Backend::FastEmbed => Ok(Box::new(fastembed::FastEmbedder::new()?)),
        }
    }
}

/// Why an embedding request failed. The distinction drives whether the worker
/// aborts (the whole backend is down) or skips one chunk and carries on.
#[derive(Debug)]
pub enum EmbedError {
    /// The backend could not be reached at all — stop trying this session.
    Unreachable(String),
    /// This specific input could not be embedded — skip it and continue.
    Skip(String),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedError::Unreachable(m) | EmbedError::Skip(m) => write!(f, "{m}"),
        }
    }
}

/// Weight applied to a cosine similarity (0–1) so semantic scores are
/// comparable to the cheap link/tag/keyword scores when the two are merged.
const SEMANTIC_WEIGHT: f32 = 4.0;

/// Minimum cosine similarity for two notes to count as semantically related.
const SIMILARITY_THRESHOLD: f32 = 0.35;

/// Generates embedding vectors for note text. Implementations run on the
/// background worker thread, so they must be `Send`.
pub trait Embedder: Send {
    /// Embed a single piece of text into a vector.
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError>;
}

/// Serialize a vector to little-endian bytes for BLOB storage.
pub fn to_blob(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Deserialize a BLOB of little-endian f32s back into a vector.
pub fn from_blob(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

struct Entry {
    path: String,
    title: String,
    unit: Vec<f32>,
}

/// In-memory note-level semantic index: one unit vector per note (the
/// normalized mean of its chunk embeddings). Queried by brute-force cosine.
pub struct SemanticIndex {
    entries: Vec<Entry>,
    by_path: HashMap<String, usize>,
}

impl SemanticIndex {
    /// Build from per-chunk vectors, aggregating to a mean vector per note.
    pub fn build(notes: &[NoteRow], chunk_vectors: Vec<(String, Vec<f32>)>) -> Self {
        // Sum chunk vectors per note.
        let mut sums: HashMap<String, (Vec<f32>, usize)> = HashMap::new();
        for (path, vector) in chunk_vectors {
            let entry = sums.entry(path).or_insert_with(|| (vec![0.0; vector.len()], 0));
            if entry.0.len() == vector.len() {
                for (acc, v) in entry.0.iter_mut().zip(vector.iter()) {
                    *acc += v;
                }
                entry.1 += 1;
            }
        }

        let title_of: HashMap<&str, &str> = notes
            .iter()
            .map(|n| (n.path.as_str(), n.title.as_str()))
            .collect();

        let mut entries = Vec::new();
        let mut by_path = HashMap::new();
        for (path, (sum, count)) in sums {
            if count == 0 {
                continue;
            }
            let mut mean: Vec<f32> = sum.iter().map(|x| x / count as f32).collect();
            normalize(&mut mean);
            let title = title_of.get(path.as_str()).copied().unwrap_or("").to_string();
            by_path.insert(path.clone(), entries.len());
            entries.push(Entry { path, title, unit: mean });
        }

        SemanticIndex { entries, by_path }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Notes most semantically similar to `path`, as merge-ready `RelatedNote`s.
    pub fn similar(&self, path: &str, limit: usize) -> Vec<RelatedNote> {
        let Some(&i) = self.by_path.get(path) else {
            return Vec::new();
        };
        let me = &self.entries[i];

        let mut out: Vec<RelatedNote> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .filter_map(|(_, other)| {
                let sim = dot(&me.unit, &other.unit);
                (sim >= SIMILARITY_THRESHOLD).then(|| RelatedNote {
                    path: other.path.clone(),
                    title: other.title.clone(),
                    score: SEMANTIC_WEIGHT * sim,
                    reason: format!("similar {:.0}%", sim * 100.0),
                })
            })
            .collect();

        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out.truncate(limit);
        out
    }
}

fn normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in vector.iter_mut() {
            *x /= norm;
        }
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(path: &str, title: &str) -> NoteRow {
        NoteRow {
            path: path.to_string(),
            title: title.to_string(),
            content: String::new(),
            tags: Vec::new(),
            links: Vec::new(),
        }
    }

    #[test]
    fn blob_round_trip() {
        let v = vec![0.5, -1.0, 3.25, 0.0];
        assert_eq!(from_blob(&to_blob(&v)), v);
    }

    #[test]
    fn similar_ranks_closest_note_first() {
        let notes = vec![
            note("/v/a.md", "A"),
            note("/v/b.md", "B"),
            note("/v/c.md", "C"),
        ];
        // A points along x; B is close to x; C is orthogonal.
        let chunk_vectors = vec![
            ("/v/a.md".to_string(), vec![1.0, 0.0]),
            ("/v/b.md".to_string(), vec![0.9, 0.1]),
            ("/v/c.md".to_string(), vec![0.0, 1.0]),
        ];
        let idx = SemanticIndex::build(&notes, chunk_vectors);
        let related = idx.similar("/v/a.md", 10);
        assert_eq!(related.first().unwrap().title, "B");
        // C is orthogonal (cosine 0) and below threshold, so it is excluded.
        assert!(related.iter().all(|r| r.title != "C"));
    }

    #[test]
    fn mean_of_chunks_defines_the_note_vector() {
        let notes = vec![note("/v/a.md", "A"), note("/v/b.md", "B")];
        let chunk_vectors = vec![
            ("/v/a.md".to_string(), vec![1.0, 0.0]),
            ("/v/a.md".to_string(), vec![0.0, 1.0]), // mean -> (0.5, 0.5)
            ("/v/b.md".to_string(), vec![1.0, 1.0]),
        ];
        let idx = SemanticIndex::build(&notes, chunk_vectors);
        // A's mean and B are colinear -> perfectly similar.
        let related = idx.similar("/v/a.md", 10);
        assert_eq!(related.len(), 1);
        assert!(related[0].score > SEMANTIC_WEIGHT * 0.99);
    }
}
