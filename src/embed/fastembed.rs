//! In-process ONNX embedding backend via the `fastembed` crate.
//!
//! Only compiled with `--features fastembed`. The model is downloaded from
//! Hugging Face on first use and cached; no Ollama daemon is required.

use std::sync::Mutex;

use ::fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use super::{EmbedError, Embedder};

/// Human-readable model identifier, used for `meta` gating and the UI.
pub const MODEL_ID: &str = "all-MiniLM-L6-v2";

/// Embedder that runs `all-MiniLM-L6-v2` (384-dim) locally on CPU.
///
/// `TextEmbedding::embed` takes `&mut self`, but the `Embedder` trait is
/// `&self`; a `Mutex` bridges the two. The worker embeds sequentially, so
/// there is no lock contention.
pub struct FastEmbedder {
    model: Mutex<TextEmbedding>,
}

impl FastEmbedder {
    pub fn new() -> Result<Self, EmbedError> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(false),
        )
        .map_err(|e| EmbedError::Unreachable(format!("initializing fastembed: {e}")))?;
        Ok(FastEmbedder {
            model: Mutex::new(model),
        })
    }
}

impl Embedder for FastEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let mut model = self
            .model
            .lock()
            .map_err(|_| EmbedError::Unreachable("fastembed model lock poisoned".to_string()))?;
        // fastembed truncates to the model's context window internally, so
        // over-long input is not an error here.
        let mut vectors = model
            .embed(vec![text], None)
            .map_err(|e| EmbedError::Skip(format!("fastembed embed failed: {e}")))?;
        vectors
            .pop()
            .ok_or_else(|| EmbedError::Skip("fastembed returned no embedding".to_string()))
    }
}
