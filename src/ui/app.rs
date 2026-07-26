use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::Text;
use ratatui::widgets::ListState;

use crate::config::RefFormat;
use crate::db::store::NoteRow;
use crate::embed::SemanticIndex;
use crate::linking;
use crate::related::{self, RelatedIndex, RelatedNote};
use crate::ui::markdown;

/// Which pane currently receives navigation keys.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Notes,
    Preview,
    Related,
    Scratchpad,
}

impl Focus {
    fn next(self) -> Focus {
        match self {
            Focus::Notes => Focus::Preview,
            Focus::Preview => Focus::Related,
            Focus::Related => Focus::Scratchpad,
            Focus::Scratchpad => Focus::Notes,
        }
    }

    fn prev(self) -> Focus {
        match self {
            Focus::Notes => Focus::Scratchpad,
            Focus::Preview => Focus::Notes,
            Focus::Related => Focus::Preview,
            Focus::Scratchpad => Focus::Related,
        }
    }
}

/// Result of handling a key, actioned by the main loop.
pub enum Action {
    None,
    Quit,
    OpenEditor(PathBuf),
    Yank(String),
    /// The scratchpad changed and should be persisted.
    ScratchpadChanged,
    /// These files were modified on disk and should be re-indexed.
    ReindexPaths(Vec<String>),
}

/// A candidate pair of notes to offer a link between, during the linking flow.
#[derive(Clone)]
struct LinkPair {
    a_path: String,
    a_title: String,
    a_name: String,
    b_path: String,
    b_title: String,
    b_name: String,
}

/// State of an in-progress "link the scratchpad notes" session.
struct LinkingState {
    pairs: Vec<LinkPair>,
    index: usize,
    created: usize,
    modified: Vec<String>,
}

/// Read-only snapshot of the current link prompt, for rendering the modal.
pub struct LinkPrompt {
    pub a_name: String,
    pub b_name: String,
    pub index: usize,
    pub total: usize,
}

/// All browsing/search state for the TUI.
pub struct App {
    /// Vault root, for rendering relative paths.
    pub vault: PathBuf,
    /// Default reference format for clipboard yanks.
    ref_format: RefFormat,
    /// Every indexed note, ordered by title.
    notes: Vec<NoteRow>,
    /// Precomputed cheap relatedness (links/tags/keywords) over the vault.
    related_index: RelatedIndex,
    /// Optional semantic index; present once embeddings have been computed.
    semantic: Option<SemanticIndex>,
    /// Related notes for the current selection (cheap + semantic, merged).
    related: Vec<RelatedNote>,
    /// Indices into `notes` matching the current query.
    filtered: Vec<usize>,
    /// Selection within `filtered`.
    selected: usize,
    /// Which pane has focus.
    pub focus: Focus,
    /// Rendered Markdown of the current note, cached so it is only built when
    /// the note changes rather than every frame.
    preview: Text<'static>,
    /// Vertical scroll offset of the preview pane.
    pub preview_scroll: u16,
    /// Selection within the related-notes pane.
    related_selected: usize,
    /// Whether the search input is capturing keystrokes.
    pub searching: bool,
    /// Current search query (also used as a persistent filter).
    pub query: String,
    /// Selection state for the notes list widget.
    pub list_state: ListState,
    /// Selection state for the related-notes list widget.
    pub related_state: ListState,
    /// The scratchpad: an ordered working set of note paths.
    scratchpad: Vec<String>,
    /// Selection within the scratchpad pane.
    scratchpad_selected: usize,
    /// Selection state for the scratchpad list widget.
    pub scratchpad_state: ListState,
    /// In-progress link-the-scratchpad session, if any.
    linking: Option<LinkingState>,
    /// Transient status-line message (e.g. a yank confirmation).
    pub status: Option<String>,
}

impl App {
    pub fn new(
        vault: PathBuf,
        ref_format: RefFormat,
        notes: Vec<NoteRow>,
        scratchpad: Vec<String>,
    ) -> Self {
        let related_index = RelatedIndex::build(&notes);
        let mut app = App {
            vault,
            ref_format,
            notes,
            related_index,
            semantic: None,
            related: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            focus: Focus::Notes,
            preview: Text::default(),
            preview_scroll: 0,
            related_selected: 0,
            searching: false,
            query: String::new(),
            list_state: ListState::default(),
            related_state: ListState::default(),
            scratchpad,
            scratchpad_selected: 0,
            scratchpad_state: ListState::default(),
            linking: None,
            status: None,
        };
        app.refilter();
        app.select_note_changed();
        app.sync_scratchpad_state();
        app
    }

    /// Replace the note set after re-indexing, preserving the selected note if possible.
    pub fn set_notes(&mut self, notes: Vec<NoteRow>) {
        let current = self.anchor_note().map(|n| n.path.clone());
        self.related_index = RelatedIndex::build(&notes);
        self.notes = notes;
        self.refilter();
        if let Some(path) = current {
            if let Some(pos) = self.filtered.iter().position(|&i| self.notes[i].path == path) {
                self.selected = pos;
                self.sync_list_state();
            }
        }
        self.select_note_changed();
    }

    /// Install (or replace) the semantic index and refresh related notes.
    pub fn set_semantic(&mut self, semantic: SemanticIndex) {
        self.semantic = if semantic.is_empty() {
            None
        } else {
            Some(semantic)
        };
        self.recompute_related();
        self.sync_related_state();
    }

    /// The vault-relative filenames (extension stripped) of the visible notes,
    /// shown in the notes list.
    pub fn visible_files(&self) -> Vec<String> {
        self.filtered
            .iter()
            .map(|&i| self.display_filename(&self.notes[i].path))
            .collect()
    }

    /// A note's vault-relative filename with the extension stripped, for display
    /// in the notes list and scratchpad.
    fn display_filename(&self, path: &str) -> String {
        let rel = self.relative_path(path);
        match rel.rsplit_once('.') {
            Some((stem, _ext)) => stem.to_string(),
            None => rel,
        }
    }

    /// Related notes for the current selection.
    pub fn related(&self) -> &[RelatedNote] {
        &self.related
    }

    /// The cached, styled Markdown of the current note.
    pub fn preview(&self) -> &Text<'static> {
        &self.preview
    }

    /// The scratchpad note paths, in order (for persistence).
    pub fn scratchpad_paths(&self) -> &[String] {
        &self.scratchpad
    }

    /// Display labels for the scratchpad entries: the filename, matching the
    /// notes list (never the derived title/heading).
    pub fn scratchpad_files(&self) -> Vec<String> {
        self.scratchpad
            .iter()
            .map(|path| self.display_filename(path))
            .collect()
    }

    /// The note anchoring the view: the Notes-pane selection. The related list
    /// is always computed from this note, so navigating the Related pane does
    /// not disturb it.
    fn anchor_note(&self) -> Option<&NoteRow> {
        self.filtered.get(self.selected).map(|&i| &self.notes[i])
    }

    /// The note the preview pane shows and that edit/yank act on: the
    /// highlighted note when the Related or Scratchpad pane has focus, otherwise
    /// the anchor. This lets navigating those panes preview notes without moving
    /// the anchor.
    pub fn active_note(&self) -> Option<&NoteRow> {
        let highlighted = match self.focus {
            Focus::Related => self.related.get(self.related_selected).map(|r| r.path.as_str()),
            Focus::Scratchpad => self
                .scratchpad
                .get(self.scratchpad_selected)
                .map(|p| p.as_str()),
            _ => None,
        };
        if let Some(path) = highlighted {
            if let Some(note) = self.notes.iter().find(|n| n.path == path) {
                return Some(note);
            }
        }
        self.anchor_note()
    }

    /// Path of the active note relative to the vault root, for display.
    pub fn current_relative_path(&self) -> Option<String> {
        self.active_note().map(|note| self.relative_path(&note.path))
    }

    fn relative_path(&self, path: &str) -> String {
        let path = PathBuf::from(path);
        path.strip_prefix(&self.vault)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string()
    }

    /// The clipboard reference for the active note in the configured format.
    fn current_reference(&self) -> Option<String> {
        let note = self.active_note()?;
        Some(match self.ref_format {
            RefFormat::Markdown => {
                let target = self.relative_path(&note.path);
                // A destination containing spaces must be wrapped in <> to stay
                // a valid CommonMark link.
                let dest = if target.contains(' ') {
                    format!("<{target}>")
                } else {
                    target
                };
                format!("[{}]({})", note.title, dest)
            }
            RefFormat::Relative => self.relative_path(&note.path),
            RefFormat::Absolute => note.path.clone(),
        })
    }

    /// Handle a key press and report any action for the main loop.
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        // Any deliberate key clears a lingering status message.
        self.status = None;

        // The linking modal captures all input while active.
        if self.linking.is_some() {
            return self.handle_linking_key(key);
        }

        if self.searching {
            match key.code {
                // First Esc leaves the input but keeps the filter; a second Esc
                // (handled below, in normal mode) clears it.
                KeyCode::Esc | KeyCode::Enter => self.searching = false,
                KeyCode::Backspace => {
                    self.query.pop();
                    self.refilter();
                    self.select_note_changed();
                }
                KeyCode::Char(c) => {
                    self.query.push(c);
                    self.refilter();
                    self.select_note_changed();
                }
                _ => {}
            }
            return Action::None;
        }

        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            // Esc clears an active search filter (the second press after leaving
            // the search input).
            KeyCode::Esc => {
                if !self.query.is_empty() {
                    self.query.clear();
                    self.refilter();
                    self.select_note_changed();
                }
            }
            KeyCode::Char('1') => self.set_focus(Focus::Notes),
            KeyCode::Char('2') => self.set_focus(Focus::Preview),
            KeyCode::Char('3') => self.set_focus(Focus::Related),
            KeyCode::Char('4') => self.set_focus(Focus::Scratchpad),
            KeyCode::Tab => self.set_focus(self.focus.next()),
            KeyCode::BackTab => self.set_focus(self.focus.prev()),
            KeyCode::Char('/') => {
                self.set_focus(Focus::Notes);
                self.searching = true;
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),
            KeyCode::Enter => return self.on_enter(),
            KeyCode::Char('a') => return self.add_to_scratchpad(),
            KeyCode::Char('x') if self.focus == Focus::Scratchpad => {
                return self.remove_from_scratchpad();
            }
            KeyCode::Char('l') => self.start_linking(),
            KeyCode::Char('y') => {
                if let Some(reference) = self.current_reference() {
                    return Action::Yank(reference);
                }
            }
            KeyCode::Char('e') => {
                if let Some(note) = self.active_note() {
                    return Action::OpenEditor(PathBuf::from(&note.path));
                }
            }
            _ => {}
        }
        Action::None
    }

    /// Add the active note to the scratchpad (ignoring duplicates).
    fn add_to_scratchpad(&mut self) -> Action {
        let Some(path) = self.active_note().map(|n| n.path.clone()) else {
            return Action::None;
        };
        let name = self.display_filename(&path);
        if self.scratchpad.contains(&path) {
            self.set_status(format!("Already in scratchpad: {name}"));
            return Action::None;
        }
        self.scratchpad.push(path);
        self.scratchpad_selected = self.scratchpad.len() - 1;
        self.sync_scratchpad_state();
        self.set_status(format!("Added to scratchpad: {name}"));
        Action::ScratchpadChanged
    }

    /// Remove the highlighted scratchpad entry.
    fn remove_from_scratchpad(&mut self) -> Action {
        if self.scratchpad.is_empty() {
            return Action::None;
        }
        let removed = self.scratchpad.remove(self.scratchpad_selected);
        self.sync_scratchpad_state();
        // The highlighted entry changed (or the list emptied); refresh preview.
        self.rebuild_preview();
        let name = self.display_filename(&removed);
        self.set_status(format!("Removed from scratchpad: {name}"));
        Action::ScratchpadChanged
    }

    /// A read-only snapshot of the current link prompt, for rendering the modal.
    pub fn linking_prompt(&self) -> Option<LinkPrompt> {
        self.linking.as_ref().map(|state| {
            let pair = &state.pairs[state.index];
            LinkPrompt {
                a_name: pair.a_name.clone(),
                b_name: pair.b_name.clone(),
                index: state.index + 1,
                total: state.pairs.len(),
            }
        })
    }

    /// Begin linking the scratchpad notes: build the queue of not-yet-linked
    /// pairs and open the modal on the first one.
    fn start_linking(&mut self) {
        if self.scratchpad.len() < 2 {
            self.set_status("Add at least 2 notes to the scratchpad to link them.");
            return;
        }

        // Resolve each scratchpad path to (path, link-text title, display name),
        // dropping any that are no longer indexed.
        let notes: Vec<(String, String, String)> = self
            .scratchpad
            .iter()
            .filter_map(|path| {
                self.notes
                    .iter()
                    .find(|n| &n.path == path)
                    .map(|n| (n.path.clone(), n.title.clone(), self.display_filename(path)))
            })
            .collect();

        let mut pairs = Vec::new();
        for i in 0..notes.len() {
            for j in (i + 1)..notes.len() {
                let (a_path, a_title, a_name) = &notes[i];
                let (b_path, b_title, b_name) = &notes[j];
                if !self.related_index.are_linked(a_path, b_path) {
                    pairs.push(LinkPair {
                        a_path: a_path.clone(),
                        a_title: a_title.clone(),
                        a_name: a_name.clone(),
                        b_path: b_path.clone(),
                        b_title: b_title.clone(),
                        b_name: b_name.clone(),
                    });
                }
            }
        }

        if pairs.is_empty() {
            self.set_status("All scratchpad notes are already linked.");
            return;
        }

        let total = pairs.len();
        self.linking = Some(LinkingState {
            pairs,
            index: 0,
            created: 0,
            modified: Vec::new(),
        });
        self.set_status(format!("Linking: {total} pair(s) to review"));
    }

    /// Handle a key while the linking modal is open.
    fn handle_linking_key(&mut self, key: KeyEvent) -> Action {
        // (a→b, b→a) directions to create for this pair; None = ignore key.
        let directions = match key.code {
            KeyCode::Char('1') => Some((true, false)),
            KeyCode::Char('2') => Some((false, true)),
            KeyCode::Char('3') => Some((true, true)),
            KeyCode::Char('4') | KeyCode::Char('n') => Some((false, false)),
            KeyCode::Esc => return self.finish_linking(true),
            _ => None,
        };
        let Some((a_to_b, b_to_a)) = directions else {
            return Action::None;
        };

        let Some(pair) = self.linking.as_ref().map(|s| s.pairs[s.index].clone()) else {
            return Action::None;
        };

        let mut modified = Vec::new();
        if a_to_b && self.apply_link(&pair.a_path, &pair.b_path, &pair.b_title) {
            modified.push(pair.a_path.clone());
        }
        if b_to_a && self.apply_link(&pair.b_path, &pair.a_path, &pair.a_title) {
            modified.push(pair.b_path.clone());
        }

        if let Some(state) = self.linking.as_mut() {
            state.created += modified.len();
            state.modified.extend(modified);
            state.index += 1;
            if state.index >= state.pairs.len() {
                return self.finish_linking(false);
            }
        }
        Action::None
    }

    /// Write one link; report failure via the status line. Returns success.
    fn apply_link(&mut self, from: &str, to: &str, to_title: &str) -> bool {
        match linking::append_link(Path::new(from), Path::new(to), to_title) {
            Ok(()) => true,
            Err(err) => {
                self.set_status(format!("Link failed: {err}"));
                false
            }
        }
    }

    /// End the linking session, reporting a summary and requesting re-index of
    /// any modified files.
    fn finish_linking(&mut self, cancelled: bool) -> Action {
        let Some(state) = self.linking.take() else {
            return Action::None;
        };
        let verb = if cancelled { "cancelled" } else { "complete" };
        self.set_status(format!(
            "Linking {verb} — {} link(s) created",
            state.created
        ));
        if state.modified.is_empty() {
            Action::None
        } else {
            Action::ReindexPaths(state.modified)
        }
    }

    /// Enter does something different depending on the focused pane.
    fn on_enter(&mut self) -> Action {
        if self.focus == Focus::Related {
            self.jump_to_related();
            return Action::None;
        }
        // Notes / Preview: open the active note in the editor.
        match self.active_note() {
            Some(note) => Action::OpenEditor(PathBuf::from(&note.path)),
            None => Action::None,
        }
    }

    /// Change the focused pane. Focusing (or leaving) the Related pane changes
    /// which note is active, so the preview is rebuilt.
    fn set_focus(&mut self, focus: Focus) {
        if self.focus != focus {
            self.focus = focus;
            self.rebuild_preview();
        }
    }

    /// Set a transient status-line message.
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
    }

    fn move_down(&mut self) {
        match self.focus {
            Focus::Notes => self.move_selection(1),
            Focus::Preview => self.scroll_preview(1),
            Focus::Related => self.move_related(1),
            Focus::Scratchpad => self.move_scratchpad(1),
        }
    }

    fn move_up(&mut self) {
        match self.focus {
            Focus::Notes => self.move_selection(-1),
            Focus::Preview => self.scroll_preview(-1),
            Focus::Related => self.move_related(-1),
            Focus::Scratchpad => self.move_scratchpad(-1),
        }
    }

    fn move_scratchpad(&mut self, delta: isize) {
        if self.scratchpad.is_empty() {
            return;
        }
        let len = self.scratchpad.len() as isize;
        self.scratchpad_selected =
            (self.scratchpad_selected as isize + delta).rem_euclid(len) as usize;
        self.scratchpad_state.select(Some(self.scratchpad_selected));
        // Preview the highlighted scratchpad note (focus is Scratchpad here).
        self.rebuild_preview();
    }

    fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            return;
        }
        let len = self.filtered.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        self.sync_list_state();
        self.select_note_changed();
    }

    fn move_related(&mut self, delta: isize) {
        if self.related.is_empty() {
            return;
        }
        let len = self.related.len() as isize;
        self.related_selected = (self.related_selected as isize + delta).rem_euclid(len) as usize;
        self.related_state.select(Some(self.related_selected));
        // The active note is now the highlighted related note; preview it.
        self.rebuild_preview();
    }

    fn scroll_preview(&mut self, delta: i32) {
        let max = self
            .active_note()
            .map(|n| n.content.lines().count() as i32)
            .unwrap_or(0);
        let next = (self.preview_scroll as i32 + delta).clamp(0, max.max(0));
        self.preview_scroll = next as u16;
    }

    /// Make the highlighted related note the current note.
    fn jump_to_related(&mut self) {
        if let Some(target) = self.related.get(self.related_selected).map(|r| r.path.clone()) {
            self.jump_to_path(&target);
        }
    }

    fn jump_to_path(&mut self, path: &str) {
        if !self.notes.iter().any(|n| n.path == path) {
            return;
        }
        // If the target is filtered out, clear the search so it is reachable.
        if !self.filtered.iter().any(|&i| self.notes[i].path == path) {
            self.query.clear();
            self.searching = false;
            self.refilter();
        }
        if let Some(pos) = self.filtered.iter().position(|&i| self.notes[i].path == path) {
            self.selected = pos;
            self.sync_list_state();
        }
        self.focus = Focus::Notes;
        self.select_note_changed();
    }

    fn refilter(&mut self) {
        let query = self.query.to_lowercase();
        self.filtered = self
            .notes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                if query.is_empty() {
                    return true;
                }
                let filename = Path::new(&n.path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                filename.contains(&query)
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

    /// Called whenever the *anchor* note changes: recompute the related list
    /// and re-render the preview.
    fn select_note_changed(&mut self) {
        self.related_selected = 0;
        self.recompute_related();
        self.sync_related_state();
        self.rebuild_preview();
    }

    /// Re-render the cached preview for the currently active note.
    fn rebuild_preview(&mut self) {
        self.preview_scroll = 0;
        self.preview = match self.active_note().map(|n| n.content.clone()) {
            Some(content) => markdown::render(&content),
            None => Text::from("No note selected."),
        };
    }

    fn recompute_related(&mut self) {
        self.related = match self.anchor_note().map(|n| n.path.clone()) {
            Some(path) => {
                let cheap = self.related_index.related(&path, 12);
                let semantic = self
                    .semantic
                    .as_ref()
                    .map(|s| s.similar(&path, 12))
                    .unwrap_or_default();
                related::merge([cheap, semantic], 12)
            }
            None => Vec::new(),
        };
    }

    fn sync_list_state(&mut self) {
        self.list_state
            .select((!self.filtered.is_empty()).then_some(self.selected));
    }

    fn sync_related_state(&mut self) {
        if self.related.is_empty() {
            self.related_selected = 0;
            self.related_state.select(None);
        } else {
            if self.related_selected >= self.related.len() {
                self.related_selected = self.related.len() - 1;
            }
            self.related_state.select(Some(self.related_selected));
        }
    }

    fn sync_scratchpad_state(&mut self) {
        if self.scratchpad.is_empty() {
            self.scratchpad_selected = 0;
            self.scratchpad_state.select(None);
        } else {
            if self.scratchpad_selected >= self.scratchpad.len() {
                self.scratchpad_selected = self.scratchpad.len() - 1;
            }
            self.scratchpad_state.select(Some(self.scratchpad_selected));
        }
    }
}
