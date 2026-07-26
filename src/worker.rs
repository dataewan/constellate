use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;

use crate::embed::{Backend, EmbedError};

/// A message from the embedding worker back to the UI thread.
pub enum WorkerMsg {
    /// A chunk was successfully embedded.
    Embedded { chunk_id: i64, vector: Vec<f32> },
    /// One chunk could not be embedded but the backend is otherwise fine;
    /// logged, not fatal.
    Skipped { chunk_id: i64, reason: String },
    /// The current batch finished (fully or after some skips).
    Done,
    /// The backend is unreachable; the message explains why. No further work
    /// should be submitted this session.
    Failed(String),
}

/// Handle to the background embedding thread. Dropping it closes the job
/// channel, which ends the thread after its current batch.
pub struct Worker {
    jobs: Sender<Vec<(i64, String)>>,
    pub results: Receiver<WorkerMsg>,
    _handle: JoinHandle<()>,
}

impl Worker {
    /// Submit a batch of `(chunk_id, text)` pairs to embed.
    pub fn submit(&self, batch: Vec<(i64, String)>) {
        let _ = self.jobs.send(batch);
    }
}

/// Spawn the embedding worker. Embedding is CPU/network-bound, so it runs off
/// the synchronous UI thread and reports results over a channel. The embedder
/// is constructed inside the thread (it may download a model or init a runtime).
pub fn spawn(backend: Backend) -> Worker {
    let (job_tx, job_rx) = channel::<Vec<(i64, String)>>();
    let (res_tx, res_rx) = channel::<WorkerMsg>();

    let handle = std::thread::spawn(move || {
        let embedder = match backend.build() {
            Ok(embedder) => embedder,
            Err(err) => {
                let _ = res_tx.send(WorkerMsg::Failed(err.to_string()));
                return;
            }
        };
        while let Ok(batch) = job_rx.recv() {
            for (chunk_id, text) in batch {
                let msg = match embedder.embed(&text) {
                    Ok(vector) => WorkerMsg::Embedded { chunk_id, vector },
                    // One bad input (e.g. over-long chunk): skip and continue.
                    Err(EmbedError::Skip(reason)) => WorkerMsg::Skipped { chunk_id, reason },
                    // Backend down: report and abandon the batch.
                    Err(EmbedError::Unreachable(reason)) => {
                        let _ = res_tx.send(WorkerMsg::Failed(reason));
                        break;
                    }
                };
                if res_tx.send(msg).is_err() {
                    return; // UI gone; stop working.
                }
            }
            let _ = res_tx.send(WorkerMsg::Done);
        }
    });

    Worker {
        jobs: job_tx,
        results: res_rx,
        _handle: handle,
    }
}
