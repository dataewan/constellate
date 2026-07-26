use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Append a line to the log file, best-effort. Used for background-worker
/// events (embedding skips/failures) that must not corrupt the TUI by going to
/// stderr. Errors here are intentionally ignored — logging must never crash the
/// app.
pub fn log_line(path: &Path, message: &str) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "[{secs}] {message}");
    }
}
