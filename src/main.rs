mod cli;
mod config;
mod db;
mod editor;
mod ui;
mod vault;
mod watch;

use std::io::{self, Stdout};
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
use crate::ui::{Action, App};

type Term = Terminal<CrosstermBackend<Stdout>>;

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

    let notes = store.all_notes()?;
    let mut app = App::new(config.vault.clone(), notes);

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
            app.set_notes(store.all_notes()?);
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
                        app.set_notes(store.all_notes()?);
                    }
                }
            }
        }
    }
    Ok(())
}
