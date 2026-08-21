//! Scratchpad → LLM synthesis: concatenate the scratchpad notes, ask a chosen
//! prompt, and write the result to a new `#TODO` note in the vault root.
//!
//! The actual model call is delegated to an [`LlmProvider`] (Ollama by default,
//! or the hosted Claude / Gemini backends), so the synthesis pipeline is
//! independent of which model produces the text.

mod claude;
mod gemini;
mod ollama;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::linking;

/// Per-note character cap when assembling the prompt, so one huge note can't
/// blow the model's context window.
const MAX_NOTE_CHARS: usize = 8000;

/// The preset prompt library: (label, prompt text). A "Custom…" entry is
/// offered by the UI in addition to these. Used to seed the DB-backed prompt
/// library on first run.
pub const PROMPTS: &[(&str, &str)] = &[
    (
        "Connections",
        "What are the connections between these notes that aren't explicitly stated?",
    ),
    (
        "Same area",
        "What other ideas are in the same area as these notes?",
    ),
    (
        "Common theme",
        "What is the common theme across these notes? Summarize it.",
    ),
    (
        "Open questions",
        "What questions do these notes raise that aren't answered?",
    ),
    (
        "What's missing",
        "What's missing from this cluster of ideas? What should I explore next?",
    ),
    (
        "Tensions",
        "Where do these notes disagree or create tension with each other?",
    ),
];

/// Which LLM backend to use for synthesis. Persisted in the `meta` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    /// Local Ollama server (default; no API key).
    Ollama,
    /// Anthropic Claude (Messages API; needs `ANTHROPIC_API_KEY`).
    Claude,
    /// Google Gemini (generateContent; needs `GEMINI_API_KEY`).
    Gemini,
}

impl ProviderKind {
    /// The stable string used in the `meta` table / CLI.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Ollama => "ollama",
            ProviderKind::Claude => "claude",
            ProviderKind::Gemini => "gemini",
        }
    }

    /// Parse the stored / CLI string, if recognized.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "ollama" => Some(ProviderKind::Ollama),
            "claude" | "anthropic" => Some(ProviderKind::Claude),
            "gemini" | "google" => Some(ProviderKind::Gemini),
            _ => None,
        }
    }

    /// A sensible default model for this backend, used to seed config.
    pub fn default_model(self) -> &'static str {
        match self {
            ProviderKind::Ollama => "qwen2.5",
            ProviderKind::Claude => "claude-opus-4-8",
            ProviderKind::Gemini => "gemini-2.5-flash",
        }
    }

    /// The environment variable a hosted backend reads its API key from, if any.
    pub fn key_env(self) -> Option<&'static str> {
        match self {
            ProviderKind::Ollama => None,
            ProviderKind::Claude => Some("ANTHROPIC_API_KEY"),
            ProviderKind::Gemini => Some("GEMINI_API_KEY"),
        }
    }

    /// Human-readable label for the config modal.
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Ollama => "Ollama (local)",
            ProviderKind::Claude => "Claude (hosted)",
            ProviderKind::Gemini => "Gemini (hosted)",
        }
    }

    /// All backends, in modal display order.
    pub const ALL: [ProviderKind; 3] = [
        ProviderKind::Ollama,
        ProviderKind::Claude,
        ProviderKind::Gemini,
    ];
}

/// A model backend that turns a prompt into text. Constructed on the synthesis
/// thread, so it must be `Send`.
pub trait LlmProvider: Send {
    /// Run the model on `prompt`, returning the generated text or an error
    /// message suitable for the status line / log.
    fn generate(&self, prompt: &str) -> Result<String, String>;
}

/// Build the provider for `kind`, reading any required API key from the
/// environment. Fails with a clear message when a hosted key is missing.
pub fn build_provider(
    kind: ProviderKind,
    model: &str,
    ollama_url: &str,
) -> Result<Box<dyn LlmProvider>, String> {
    match kind {
        ProviderKind::Ollama => Ok(Box::new(ollama::OllamaProvider::new(
            ollama_url.to_string(),
            model.to_string(),
        ))),
        ProviderKind::Claude => {
            let key = require_key(kind)?;
            Ok(Box::new(claude::ClaudeProvider::new(
                key,
                model.to_string(),
            )))
        }
        ProviderKind::Gemini => {
            let key = require_key(kind)?;
            Ok(Box::new(gemini::GeminiProvider::new(
                key,
                model.to_string(),
            )))
        }
    }
}

fn require_key(kind: ProviderKind) -> Result<String, String> {
    let env = kind.key_env().expect("hosted backend has a key env var");
    std::env::var(env).map_err(|_| {
        format!(
            "{} requires the {env} environment variable to be set",
            kind.label()
        )
    })
}

/// A synthesis job: the chosen prompt plus the scratchpad notes to feed in.
pub struct SynthesisRequest {
    pub prompt_text: String,
    /// (note path, note title) for each scratchpad note.
    pub sources: Vec<(PathBuf, String)>,
}

/// Run a synthesis end to end on the calling (worker) thread: read the notes,
/// call the model via `provider`, and write the output note. Returns the created
/// file path, or an error message suitable for the status line / log.
pub fn run_synthesis(
    provider: &dyn LlmProvider,
    vault: &Path,
    request: &SynthesisRequest,
) -> Result<PathBuf, String> {
    let prompt = build_prompt(&request.sources, &request.prompt_text)?;
    let output = provider.generate(&prompt)?;
    write_synthesis(vault, &request.sources, &output)
}

fn build_prompt(sources: &[(PathBuf, String)], question: &str) -> Result<String, String> {
    let mut doc = String::new();
    for (path, title) in sources {
        let content =
            fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        let excerpt: String = content.trim().chars().take(MAX_NOTE_CHARS).collect();
        doc.push_str(&format!("### {title}\n\n{excerpt}\n\n"));
    }
    Ok(format!(
        "You are helping connect and synthesize a set of personal notes. Read the \
         notes below, then complete the task.\n\n\
         --- NOTES ---\n{doc}--- END NOTES ---\n\n\
         Task: {question}\n\n\
         Write a clear, well-structured Markdown response."
    ))
}

/// Write the synthesis note to the vault root: `#TODO`, then links to the
/// source notes, then the model output. Returns the created path. The filename
/// slug is taken from the first source note's filename (its topic).
fn write_synthesis(
    vault: &Path,
    sources: &[(PathBuf, String)],
    output: &str,
) -> Result<PathBuf, String> {
    let slug = sources
        .first()
        .map(|(path, _title)| slug_from_filename(path))
        .unwrap_or_else(|| "synthesis".to_string());
    let base = format!("{}-{}", timestamp_id(now_secs()), slug);

    // Avoid clobbering an existing note.
    let mut path = vault.join(format!("{base}.md"));
    let mut counter = 2;
    while path.exists() {
        path = vault.join(format!("{base}-{counter}.md"));
        counter += 1;
    }

    let mut content = String::from("#TODO\n\n");
    for (source, title) in sources {
        let rel = linking::relative_path(vault, source);
        content.push_str(&linking::markdown_link(title, &rel.to_string_lossy()));
        content.push('\n');
    }
    content.push('\n');
    content.push_str(output.trim());
    content.push('\n');

    fs::write(&path, content).map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(path)
}

/// Slug from a note's filename stem (its topic), with any leading Zettelkasten
/// timestamp dropped so it doesn't add a second datestamp — e.g.
/// `202111211733-tony-blair.md` → `tony-blair`.
fn slug_from_filename(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    slugify(strip_leading_id(&stem))
}

/// Drop a leading id of 8+ digits (and any following separator), so a source
/// note named `202111211733-tony-blair` contributes `tony-blair`. Shorter
/// leading numbers (e.g. `3-ideas`) are left alone.
fn strip_leading_id(stem: &str) -> &str {
    let digits = stem.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits >= 8 {
        let rest = stem[digits..].trim_start_matches(['-', '_', ' ', '.']);
        if !rest.is_empty() {
            return rest;
        }
    }
    stem
}

/// Lowercase, alphanumeric-only slug with runs of other characters collapsed to
/// single dashes.
fn slugify(text: &str) -> String {
    let mut result = String::new();
    let mut pending_dash = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if pending_dash && !result.is_empty() {
                result.push('-');
            }
            result.extend(ch.to_lowercase());
            pending_dash = false;
        } else {
            pending_dash = true;
        }
    }
    if result.is_empty() {
        result.push_str("synthesis");
    }
    result
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A `YYYYMMDDHHMM` timestamp id (UTC) from seconds since the epoch, e.g.
/// `202111211733`.
fn timestamp_id(secs: i64) -> String {
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let seconds_of_day = secs.rem_euclid(86_400);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    format!("{year:04}{month:02}{day:02}{hour:02}{minute:02}")
}

/// Convert days since the Unix epoch to a (year, month, day) civil date.
/// Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("What's missing / next"), "what-s-missing-next");
        assert_eq!(slugify("Connections"), "connections");
        assert_eq!(slugify("!!!"), "synthesis");
    }

    #[test]
    fn strips_leading_zettelkasten_id() {
        assert_eq!(
            strip_leading_id("202111211733-tony-blair-lack"),
            "tony-blair-lack"
        );
        assert_eq!(strip_leading_id("20211121_foo"), "foo");
        assert_eq!(strip_leading_id("3-ideas"), "3-ideas"); // short number kept
        assert_eq!(strip_leading_id("plain-note"), "plain-note");
        assert_eq!(strip_leading_id("202111211733"), "202111211733"); // all-digits kept
    }

    #[test]
    fn known_epoch_date() {
        // 2021-01-01 is 18628 days after the epoch.
        assert_eq!(civil_from_days(18628), (2021, 1, 1));
    }

    #[test]
    fn timestamp_id_format() {
        // 2021-01-01 00:00:00 UTC.
        assert_eq!(timestamp_id(1_609_459_200), "202101010000");
        // + 17h33m.
        assert_eq!(
            timestamp_id(1_609_459_200 + 17 * 3600 + 33 * 60),
            "202101011733"
        );
    }

    #[test]
    fn provider_kind_roundtrip() {
        for kind in ProviderKind::ALL {
            assert_eq!(ProviderKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(ProviderKind::parse("anthropic"), Some(ProviderKind::Claude));
        assert_eq!(ProviderKind::parse("nonsense"), None);
    }

    #[test]
    fn hosted_backends_need_a_key_env() {
        assert_eq!(ProviderKind::Ollama.key_env(), None);
        assert_eq!(ProviderKind::Claude.key_env(), Some("ANTHROPIC_API_KEY"));
        assert_eq!(ProviderKind::Gemini.key_env(), Some("GEMINI_API_KEY"));
    }
}
