//! Scratchpad → LLM synthesis: concatenate the scratchpad notes, ask a chosen
//! prompt, and write the result to a new `#TODO` note in the vault root.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde::Deserialize;

use crate::linking;

/// Per-note character cap when assembling the prompt, so one huge note can't
/// blow the model's context window.
const MAX_NOTE_CHARS: usize = 8000;

/// The preset prompt library: (label, prompt text). A "Custom…" entry is
/// offered by the UI in addition to these.
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

/// A synthesis job: the chosen prompt plus the scratchpad notes to feed in.
pub struct SynthesisRequest {
    pub prompt_text: String,
    /// (note path, note title) for each scratchpad note.
    pub sources: Vec<(PathBuf, String)>,
}

/// Run a synthesis end to end on the calling (worker) thread: read the notes,
/// call the model, and write the output note. Returns the created file path, or
/// an error message suitable for the status line / log.
pub fn run_synthesis(
    url: &str,
    model: &str,
    vault: &Path,
    request: &SynthesisRequest,
) -> Result<PathBuf, String> {
    let prompt = build_prompt(&request.sources, &request.prompt_text)?;
    let output = generate(url, model, &prompt)?;
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

#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
}

fn generate(url: &str, model: &str, prompt: &str) -> Result<String, String> {
    let endpoint = format!("{}/api/generate", url.trim_end_matches('/'));
    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .post(&endpoint)
        .json(&serde_json::json!({ "model": model, "prompt": prompt, "stream": false }))
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

/// Slug from a note's filename stem (its topic), e.g. `tony-blair-lack.md` →
/// `tony-blair-lack`.
fn slug_from_filename(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    slugify(&stem)
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
    fn known_epoch_date() {
        // 2021-01-01 is 18628 days after the epoch.
        assert_eq!(civil_from_days(18628), (2021, 1, 1));
    }

    #[test]
    fn timestamp_id_format() {
        // 2021-01-01 00:00:00 UTC.
        assert_eq!(timestamp_id(1_609_459_200), "202101010000");
        // + 17h33m.
        assert_eq!(timestamp_id(1_609_459_200 + 17 * 3600 + 33 * 60), "202101011733");
    }
}
