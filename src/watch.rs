use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};

/// Handle that keeps the filesystem watcher alive. Dropping it stops watching.
pub type WatchHandle = Debouncer<RecommendedWatcher, RecommendedCache>;

/// Start a debounced recursive watch of the vault. Returns the keep-alive handle
/// and a receiver that yields batches of changed paths.
pub fn watch(vault: &Path) -> Result<(WatchHandle, Receiver<Vec<PathBuf>>)> {
    let (tx, rx) = channel::<Vec<PathBuf>>();

    let mut debouncer = new_debouncer(
        Duration::from_millis(400),
        None,
        move |result: DebounceEventResult| {
            if let Ok(events) = result {
                let mut paths = Vec::new();
                for event in events {
                    paths.extend(event.paths.iter().cloned());
                }
                if !paths.is_empty() {
                    let _ = tx.send(paths);
                }
            }
        },
    )?;

    debouncer.watch(vault, RecursiveMode::Recursive)?;
    Ok((debouncer, rx))
}
