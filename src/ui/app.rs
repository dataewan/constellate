use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::ListState;

use crate::db::store::NoteRow;

/// Result of handling a key, actioned by the main loop.
pub enum Action {
    None,
    Quit,
    OpenEditor(PathBuf),
}

/// All browsing/search state for the TUI.
pub struct App {
    /// Vault root, for rendering relative paths.
    pub vault: PathBuf,
    /// Every indexed note, ordered by title.
    notes: Vec<NoteRow>,
    /// Indices into `notes` matching the current query.
    filtered: Vec<usize>,
    /// Selection within `filtered`.
    selected: usize,
    /// Whether the search input is capturing keystrokes.
    pub searching: bool,
    /// Current search query (also used as a persistent filter).
    pub query: String,
    /// Selection state for the list widget.
    pub list_state: ListState,
}

impl App {
    pub fn new(vault: PathBuf, notes: Vec<NoteRow>) -> Self {
        let mut app = App {
            vault,
            notes,
            filtered: Vec::new(),
            selected: 0,
            searching: false,
            query: String::new(),
            list_state: ListState::default(),
        };
        app.refilter();
        app
    }

    /// Replace the note set after re-indexing, preserving the selected note if possible.
    pub fn set_notes(&mut self, notes: Vec<NoteRow>) {
        let current = self.current_note().map(|n| n.path.clone());
        self.notes = notes;
        self.refilter();
        if let Some(path) = current {
            if let Some(pos) = self.filtered.iter().position(|&i| self.notes[i].path == path) {
                self.selected = pos;
                self.sync_list_state();
            }
        }
    }

    /// The titles currently shown in the list pane.
    pub fn visible_titles(&self) -> Vec<&str> {
        self.filtered
            .iter()
            .map(|&i| self.notes[i].title.as_str())
            .collect()
    }

    /// The currently selected note, if any.
    pub fn current_note(&self) -> Option<&NoteRow> {
        self.filtered.get(self.selected).map(|&i| &self.notes[i])
    }

    /// Path of the selected note relative to the vault root, for display.
    pub fn current_relative_path(&self) -> Option<String> {
        let note = self.current_note()?;
        let path = PathBuf::from(&note.path);
        let rel = path.strip_prefix(&self.vault).unwrap_or(&path);
        Some(rel.to_string_lossy().to_string())
    }

    fn current_path(&self) -> Option<PathBuf> {
        self.current_note().map(|n| PathBuf::from(&n.path))
    }

    /// Handle a key press and report any action for the main loop.
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if self.searching {
            match key.code {
                KeyCode::Esc => {
                    self.searching = false;
                    self.query.clear();
                    self.refilter();
                }
                KeyCode::Enter => self.searching = false,
                KeyCode::Backspace => {
                    self.query.pop();
                    self.refilter();
                }
                KeyCode::Char(c) => {
                    self.query.push(c);
                    self.refilter();
                }
                _ => {}
            }
            return Action::None;
        }

        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('/') => self.searching = true,
            KeyCode::Char('e') | KeyCode::Enter => {
                if let Some(path) = self.current_path() {
                    return Action::OpenEditor(path);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            return;
        }
        let len = self.filtered.len() as isize;
        let next = (self.selected as isize + delta).rem_euclid(len);
        self.selected = next as usize;
        self.sync_list_state();
    }

    fn refilter(&mut self) {
        let query = self.query.to_lowercase();
        self.filtered = self
            .notes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                query.is_empty()
                    || n.title.to_lowercase().contains(&query)
                    || n.content.to_lowercase().contains(&query)
            })
            .map(|(i, _)| i)
            .collect();

        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
        self.sync_list_state();
    }

    fn sync_list_state(&mut self) {
        self.list_state
            .select((!self.filtered.is_empty()).then_some(self.selected));
    }
}
