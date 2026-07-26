mod cli;
mod clipboard;
mod config;
mod db;
mod editor;
mod embed;
mod linking;
mod llm;
mod logging;
mod related;
mod ui;
mod vault;
mod watch;
mod worker;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

use crate::cli::Cli;
use crate::config::Config;
use crate::db::store::Store;
use crate::embed::SemanticIndex;
use crate::ui::{Action, App};
use crate::worker::{Worker, WorkerMsg};

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Build the note-level semantic index from whatever embeddings are cached.
fn build_semantic(store: &Store, notes: &[db::store::NoteRow]) -> Result<SemanticIndex> {
    Ok(SemanticIndex::build(notes, store.all_chunk_vectors()?))
}

/// Send any chunks still lacking embeddings to the worker.
fn submit_pending(store: &Store, worker: &Worker) -> Result<()> {
    let pending = store.chunks_without_embeddings()?;
    if !pending.is_empty() {
        worker.submit(pending);
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::from_cli(cli)?;

    let mut store = Store::open(&config.db_path)?;

    // Initial index pass before entering the TUI.
    let stats = vault::sync_all(&mut store, &config.vault)?;
    eprintln!(
        "Indexed {} note(s) ({} new/changed, {} unchanged, {} removed) from {}",
        stats.parsed + stats.skipped,
        stats.parsed,
        stats.skipped,
        stats.deleted,
        config.vault.display()
    );

    // Reconcile the embedding backend before loading, so a model change clears
    // stale vectors up front.
    if let Some(backend) = &config.embed_backend {
        store.reconcile_embedding_backend(&backend.model_id())?;
        eprintln!(
            "Embeddings: {} (computed in the background)",
            backend.describe()
        );
    }

    let notes = store.all_notes()?;
    // Seed the semantic index from any embeddings cached in a previous run.
    let semantic = build_semantic(&store, &notes)?;
    let scratchpad = store.load_scratchpad()?;
    let mut app = App::new(config.vault.clone(), config.ref_format, notes, scratchpad);
    app.set_semantic(semantic);

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, &mut app, &mut store, &config);
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Term) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run(terminal: &mut Term, app: &mut App, store: &mut Store, config: &Config) -> Result<()> {
    // Keep the watch handle alive for the duration of the loop.
    let (_watch_handle, rx) = watch::watch(&config.vault)?;

    // Spawn the embedding worker and queue the initial backlog.
    let worker = config.embed_backend.clone().map(worker::spawn);
    let mut embed_failed = false;

    // Receiver for a background LLM synthesis, if one is running.
    let mut synthesis: Option<std::sync::mpsc::Receiver<Result<PathBuf, String>>> = None;
    if let Some(w) = &worker {
        submit_pending(store, w)?;
    }

    loop {
        terminal.draw(|f| ui::render(f, app))?;

        // Apply any filesystem changes reported by the watcher.
        let mut changed = false;
        while let Ok(paths) = rx.try_recv() {
            if vault::sync_paths(store, &config.vault, &paths)? {
                changed = true;
            }
        }
        if changed {
            let notes = store.all_notes()?;
            let semantic = build_semantic(store, &notes)?;
            app.set_notes(notes);
            app.set_semantic(semantic);
            if let Some(w) = &worker {
                if !embed_failed {
                    submit_pending(store, w)?;
                }
            }
        }

        // Drain embedding results; rebuild the semantic index once a batch ends.
        if let Some(w) = &worker {
            let mut batch_done = false;
            while let Ok(msg) = w.results.try_recv() {
                match msg {
                    WorkerMsg::Embedded { chunk_id, vector } => {
                        store.store_embedding(chunk_id, &vector)?;
                    }
                    WorkerMsg::Skipped { chunk_id, reason } => {
                        logging::log_line(
                            &config.log_path,
                            &format!("skipped embedding for chunk {chunk_id}: {reason}"),
                        );
                    }
                    WorkerMsg::Done => batch_done = true,
                    WorkerMsg::Failed(err) => {
                        embed_failed = true;
                        logging::log_line(&config.log_path, &format!("embeddings disabled: {err}"));
                        app.set_status(format!(
                            "Embeddings unavailable (is Ollama running?) — see {}",
                            config.log_path.display()
                        ));
                    }
                }
            }
            if batch_done {
                let notes = store.all_notes()?;
                app.set_semantic(build_semantic(store, &notes)?);
            }
        }

        // Collect a finished LLM synthesis, if any.
        if let Some(rx) = &synthesis {
            if let Ok(outcome) = rx.try_recv() {
                match outcome {
                    Ok(path) => {
                        let name = path
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let path_str = path.to_string_lossy().to_string();
                        if vault::sync_paths(store, &config.vault, &[path])? {
                            let notes = store.all_notes()?;
                            let semantic = build_semantic(store, &notes)?;
                            app.set_notes(notes);
                            app.set_semantic(semantic);
                            if let Some(w) = &worker {
                                if !embed_failed {
                                    submit_pending(store, w)?;
                                }
                            }
                        }
                        // Add the new note to the scratchpad.
                        if app.add_path_to_scratchpad(path_str) {
                            store.save_scratchpad(app.scratchpad_paths())?;
                        }
                        app.set_status(format!("Created synthesis: {name}"));
                    }
                    Err(err) => {
                        logging::log_line(&config.log_path, &format!("synthesis failed: {err}"));
                        app.set_status(format!("Synthesis failed: {err}"));
                    }
                }
                synthesis = None;
            }
        }

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match app.on_key(key) {
                Action::None => {}
                Action::Quit => break,
                Action::OpenEditor(path) => {
                    editor::open(terminal, &path)?;
                    // Reflect any edits immediately, then refresh the view.
                    if vault::sync_paths(store, &config.vault, &[path])? {
                        let notes = store.all_notes()?;
                        let semantic = build_semantic(store, &notes)?;
                        app.set_notes(notes);
                        app.set_semantic(semantic);
                        if let Some(w) = &worker {
                            if !embed_failed {
                                submit_pending(store, w)?;
                            }
                        }
                    }
                }
                Action::Yank(reference) => match clipboard::copy(&reference) {
                    Ok(()) => app.set_status(format!("Copied: {reference}")),
                    Err(err) => app.set_status(format!("Clipboard error: {err}")),
                },
                Action::ScratchpadChanged => store.save_scratchpad(app.scratchpad_paths())?,
                Action::GenerateSynthesis(request) => {
                    if synthesis.is_some() {
                        app.set_status("A synthesis is already running…");
                    } else {
                        let url = config.ollama_url.clone();
                        let model = config.llm_model.clone();
                        let vault = config.vault.clone();
                        let (tx, rx) = std::sync::mpsc::channel();
                        std::thread::spawn(move || {
                            let _ = tx.send(llm::run_synthesis(&url, &model, &vault, &request));
                        });
                        synthesis = Some(rx);
                        app.set_status(format!("Generating synthesis with {}…", config.llm_model));
                    }
                }
                Action::ReindexPaths(paths) => {
                    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
                    if vault::sync_paths(store, &config.vault, &paths)? {
                        let notes = store.all_notes()?;
                        let semantic = build_semantic(store, &notes)?;
                        app.set_notes(notes);
                        app.set_semantic(semantic);
                        if let Some(w) = &worker {
                            if !embed_failed {
                                submit_pending(store, w)?;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
