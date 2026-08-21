//! Google Gemini backend (`generateContent`).
//!
//! Raw HTTP via `reqwest`. The API key is read from `GEMINI_API_KEY`
//! (see `super::require_key`) and never stored.

use std::time::Duration;

use reqwest::blocking::Client;
use serde::Deserialize;

use super::{Effort, LlmProvider};

const BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// Generates text via the Gemini `generateContent` endpoint.
pub struct GeminiProvider {
    api_key: String,
    model: String,
    effort: Effort,
}

impl GeminiProvider {
    pub fn new(api_key: String, model: String, effort: Effort) -> Self {
        GeminiProvider {
            api_key,
            model,
            effort,
        }
    }
}

#[derive(Deserialize)]
struct Part {
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct Content {
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Deserialize)]
struct Candidate {
    content: Option<Content>,
}

#[derive(Deserialize)]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
}

impl LlmProvider for GeminiProvider {
    fn generate(&self, prompt: &str) -> Result<String, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| e.to_string())?;

        let endpoint = format!("{BASE}/{}:generateContent", self.model);
        let response = client
            .post(&endpoint)
            // The key rides in a header rather than the query string so it never
            // lands in request logs.
            .header("x-goog-api-key", &self.api_key)
            .json(&serde_json::json!({
                "contents": [{ "parts": [{ "text": prompt }] }],
                "generationConfig": {
                    "thinkingConfig": { "thinkingBudget": self.effort.gemini_budget() },
                },
            }))
            .send()
            .map_err(|e| format!("requesting Gemini: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(format!("Gemini returned {status}: {}", body.trim()));
        }

        let body: GenerateContentResponse = response
            .json()
            .map_err(|e| format!("decoding Gemini response: {e}"))?;

        let text = body
            .candidates
            .first()
            .and_then(|c| c.content.as_ref())
            .map(|content| {
                content
                    .parts
                    .iter()
                    .map(|p| p.text.as_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default()
            .trim()
            .to_string();

        if text.is_empty() {
            return Err("Gemini returned an empty response".to_string());
        }
        Ok(text)
    }
}
