use std::io::{self, Stdout};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Suspend the TUI, open `path` in the user's `$EDITOR`, then restore the TUI.
///
/// Resolution order: `$VISUAL`, `$EDITOR`, then `vi`. The editor command may
/// include arguments (e.g. `code --wait`).
pub fn open(terminal: &mut Term, path: &Path) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("vi");
    let args: Vec<&str> = parts.collect();

    // Hand the terminal back to the child process.
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;

    let status = Command::new(program)
        .args(&args)
        .arg(path)
        .status()
        .with_context(|| format!("failed to launch editor '{editor}'"));

    // Always restore the TUI, even if the editor failed to launch.
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    terminal.clear()?;

    status?;
    Ok(())
}
