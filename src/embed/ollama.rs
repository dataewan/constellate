use std::time::Duration;

use reqwest::blocking::Client;
use serde::Deserialize;

use super::{EmbedError, Embedder};

/// Character cap on embedding input. Embedding models have a fixed context
/// window (nomic-embed-text is 2048 tokens); over-long input makes Ollama
/// return a 500. 4000 chars stays under that even for dense/low-entropy text
/// (empirically ~5000 chars is the limit), so long note sections are truncated
/// rather than rejected. Anything still too long is skipped gracefully.
const MAX_INPUT_CHARS: usize = 4000;

/// Embedder backed by a local Ollama server's `/api/embeddings` endpoint.
pub struct OllamaEmbedder {
    client: Client,
    url: String,
    model: String,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    embedding: Vec<f32>,
}

impl OllamaEmbedder {
    pub fn new(base_url: String, model: String) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        OllamaEmbedder {
            client,
            url: base_url.trim_end_matches('/').to_string(),
            model,
        }
    }
}

impl Embedder for OllamaEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let input = truncate_chars(text, MAX_INPUT_CHARS);
        let endpoint = format!("{}/api/embeddings", self.url);

        // A send error means the server is unreachable (connection refused,
        // DNS, timeout) — that is a whole-backend failure, so abort.
        let response = self
            .client
            .post(&endpoint)
            .json(&serde_json::json!({ "model": self.model, "prompt": input }))
            .send()
            .map_err(|e| EmbedError::Unreachable(format!("{endpoint}: {e}")))?;

        // An error status (e.g. 500 "input exceeds context length") is specific
        // to this input — skip it, don't disable embeddings globally.
        let response = response.error_for_status().map_err(|e| {
            let status = e.status().map(|s| s.to_string()).unwrap_or_default();
            EmbedError::Skip(format!("HTTP {status}"))
        })?;

        let body: EmbeddingResponse = response
            .json()
            .map_err(|e| EmbedError::Skip(format!("decoding response: {e}")))?;

        if body.embedding.is_empty() {
            return Err(EmbedError::Skip("empty embedding".to_string()));
        }
        Ok(body.embedding)
    }
}

/// Truncate to at most `max` characters without splitting a UTF-8 boundary.
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect()
    }
}
