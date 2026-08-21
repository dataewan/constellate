//! Anthropic Claude backend (Messages API, `POST /v1/messages`).
//!
//! Raw HTTP via `reqwest` — there is no official Anthropic Rust SDK. The API key
//! is read from `ANTHROPIC_API_KEY` (see `super::require_key`) and never stored.

use std::time::Duration;

use reqwest::blocking::Client;
use serde::Deserialize;

use super::{Effort, LlmProvider};

const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
/// Upper bound on generated tokens for a synthesis.
const MAX_TOKENS: u32 = 4096;

/// Generates text via the Anthropic Messages API.
pub struct ClaudeProvider {
    api_key: String,
    model: String,
    effort: Effort,
}

impl ClaudeProvider {
    pub fn new(api_key: String, model: String, effort: Effort) -> Self {
        ClaudeProvider {
            api_key,
            model,
            effort,
        }
    }
}

/// A `content` block in the response; we only care about `text` blocks.
#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct MessagesResponse {
    #[serde(default)]
    content: Vec<ContentBlock>,
}

impl LlmProvider for ClaudeProvider {
    fn generate(&self, prompt: &str) -> Result<String, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| e.to_string())?;

        let response = client
            .post(ENDPOINT)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .json(&serde_json::json!({
                "model": self.model,
                "max_tokens": MAX_TOKENS,
                "output_config": { "effort": self.effort.claude_str() },
                "messages": [{ "role": "user", "content": prompt }],
            }))
            .send()
            .map_err(|e| format!("requesting Claude: {e}"))?;

        // Surface the API's error body rather than a bare status code.
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(format!("Claude returned {status}: {}", body.trim()));
        }

        let body: MessagesResponse = response
            .json()
            .map_err(|e| format!("decoding Claude response: {e}"))?;

        let text = body
            .content
            .iter()
            .filter(|b| b.kind == "text")
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("")
            .trim()
            .to_string();

        if text.is_empty() {
            return Err("Claude returned an empty response".to_string());
        }
        Ok(text)
    }
}
