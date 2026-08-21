//! Local Ollama chat backend (`/api/generate`).

use std::time::Duration;

use reqwest::blocking::Client;
use serde::Deserialize;

use super::LlmProvider;

/// Generates text via a local Ollama server.
pub struct OllamaProvider {
    url: String,
    model: String,
}

impl OllamaProvider {
    pub fn new(url: String, model: String) -> Self {
        OllamaProvider { url, model }
    }
}

#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
}

impl LlmProvider for OllamaProvider {
    fn generate(&self, prompt: &str) -> Result<String, String> {
        let endpoint = format!("{}/api/generate", self.url.trim_end_matches('/'));
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| e.to_string())?;

        let response = client
            .post(&endpoint)
            .json(&serde_json::json!({
                "model": self.model,
                "prompt": prompt,
                "stream": false,
            }))
            .send()
            .map_err(|e| format!("requesting {endpoint}: {e}"))?
            .error_for_status()
            .map_err(|e| format!("model returned an error: {e}"))?;

        let body: GenerateResponse = response
            .json()
            .map_err(|e| format!("decoding response: {e}"))?;

        let text = body.response.trim().to_string();
        if text.is_empty() {
            return Err("model returned an empty response".to_string());
        }
        Ok(text)
    }
}
