use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::Text;
use ratatui::widgets::ListState;

use crate::config::RefFormat;
use crate::db::store::NoteRow;
use crate::embed::SemanticIndex;
use crate::linking;
use crate::llm::{self, Effort, ProviderKind, SynthesisRequest};
use crate::related::{self, RelatedIndex, RelatedNote};
use crate::rename;
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
    /// Run an LLM synthesis over the scratchpad on a background thread.
    GenerateSynthesis(SynthesisRequest),
    /// Rename the active note's slug to `new_slug` (keeping its timestamp).
    RenameNote {
        path: PathBuf,
        new_slug: String,
    },
    /// The LLM provider/model config changed and should be persisted.
    LlmConfigChanged,
    /// Add a new prompt to the library.
    PromptAdd {
        label: String,
        text: String,
    },
    /// Update an existing prompt.
    PromptUpdate {
        id: i64,
        label: String,
        text: String,
    },
    /// Delete a prompt from the library.
    PromptDelete {
        id: i64,
    },
    /// Swap two prompts' positions (reorder).
    PromptReorder {
        id_a: i64,
        id_b: i64,
    },
}

/// The scratchpad → LLM prompt picker: choose a preset or enter a custom prompt.
struct PromptPicker {
    /// 0..PROMPTS.len() selects a preset; the last row selects "Custom…".
    selected: usize,
    /// `Some(text)` when in the custom free-text input mode.
    custom: Option<String>,
}

/// Read-only view of the prompt picker, for rendering the modal.
pub enum PromptPickerView {
    List {
        labels: Vec<String>,
        selected: usize,
    },
    Custom {
        text: String,
    },
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

/// Rows in the LLM config modal.
const CONFIG_ROWS: usize = 4;
const CONFIG_ROW_PROVIDER: usize = 0;
const CONFIG_ROW_MODEL: usize = 1;
const CONFIG_ROW_EFFORT: usize = 2;
const CONFIG_ROW_PROMPTS: usize = 3;

/// State of the open LLM config modal.
struct ConfigModal {
    /// Highlighted row (0 provider, 1 model, 2 prompts).
    selected: usize,
    /// `Some(buffer)` while the model field is being edited.
    editing_model: Option<String>,
}

/// Which field of a prompt is being edited in the prompt-admin editor.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptField {
    Label,
    Text,
}

/// An in-progress add/edit of a single prompt.
struct PromptEdit {
    /// `Some(id)` when editing an existing prompt; `None` when adding.
    id: Option<i64>,
    field: PromptField,
    label: String,
    text: String,
}

/// State of the open prompt-library manager (reached from the config modal).
struct PromptAdmin {
    /// Highlighted row in the prompt list.
    selected: usize,
    /// `Some` while adding or editing a prompt.
    editing: Option<PromptEdit>,
}

/// Read-only snapshot of the prompt manager, for rendering.
pub enum PromptAdminView {
    /// Browsing the list: `(label, is_selected)` per row.
    List {
        prompts: Vec<String>,
        selected: usize,
    },
    /// Editing/adding a prompt.
    Edit {
        adding: bool,
        label: String,
        text: String,
        /// Whether the label field (true) or text field (false) is active.
        editing_label: bool,
    },
}

/// Read-only snapshot of the config modal, for rendering.
pub struct ConfigView {
    pub provider_label: &'static str,
    /// The model for the selected provider (the edit buffer while editing).
    pub model: String,
    pub editing_model: bool,
    /// The reasoning-effort label for the active provider, or `None` when the
    /// active provider (Ollama) has no effort control.
    pub effort: Option<&'static str>,
    pub selected: usize,
    pub prompt_count: usize,
}

/// An in-progress "rename this note" session: the note being renamed and the
/// editable slug buffer (the timestamp prefix is fixed and not part of it).
struct RenameState {
    path: PathBuf,
    prefix: String,
    slug: String,
}

/// Read-only snapshot of the rename modal, for rendering.
pub struct RenamePrompt {
    /// The fixed `YYYYMMDDHHMM` prefix, shown before the editable slug.
    pub prefix: String,
    /// The current editable slug text.
    pub slug: String,
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
    /// The note path the related list is computed from (the "note of interest":
    /// the scratchpad highlight while pane 4 is focused, else the anchor).
    related_basis: Option<String>,
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
    /// Open prompt picker for LLM synthesis, if any.
    prompt_picker: Option<PromptPicker>,
    /// In-progress rename-the-active-note session, if any.
    renaming: Option<RenameState>,
    /// Open LLM config modal, if any.
    config_modal: Option<ConfigModal>,
    /// Open prompt-library manager, if any.
    prompt_admin: Option<PromptAdmin>,
    /// The synthesis prompt library, loaded from the DB.
    prompts: Vec<llm::Prompt>,
    /// Index into `ProviderKind::ALL` of the active provider.
    llm_provider_idx: usize,
    /// The model for each provider, in `ProviderKind::ALL` order.
    llm_models: [String; 3],
    /// Reasoning-effort level for hosted providers (Ollama ignores it).
    llm_effort: Effort,
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
            related_basis: None,
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
            prompt_picker: None,
            renaming: None,
            config_modal: None,
            prompt_admin: None,
            prompts: Vec::new(),
            llm_provider_idx: 0,
            llm_models: [
                ProviderKind::ALL[0].default_model().to_string(),
                ProviderKind::ALL[1].default_model().to_string(),
                ProviderKind::ALL[2].default_model().to_string(),
            ],
            llm_effort: Effort::default(),
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
            if let Some(pos) = self
                .filtered
                .iter()
                .position(|&i| self.notes[i].path == path)
            {
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
            Focus::Related => self
                .related
                .get(self.related_selected)
                .map(|r| r.path.as_str()),
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
        self.active_note()
            .map(|note| self.relative_path(&note.path))
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

        // Modals capture all input while active.
        if self.linking.is_some() {
            return self.handle_linking_key(key);
        }
        if self.prompt_picker.is_some() {
            return self.handle_prompt_key(key);
        }
        if self.renaming.is_some() {
            return self.handle_rename_key(key);
        }
        if self.prompt_admin.is_some() {
            return self.handle_prompt_admin_key(key);
        }
        if self.config_modal.is_some() {
            return self.handle_config_key(key);
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
            KeyCode::Char('s') => self.start_synthesis(),
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
            KeyCode::Char('r') => self.start_rename(),
            KeyCode::Char('c') => self.open_config(),
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
        if !self.add_path_to_scratchpad(path) {
            self.set_status(format!("Already in scratchpad: {name}"));
            return Action::None;
        }
        // Highlight the just-added note.
        self.scratchpad_selected = self.scratchpad.len() - 1;
        self.sync_scratchpad_state();
        self.set_status(format!("Added to scratchpad: {name}"));
        Action::ScratchpadChanged
    }

    /// Append a note path to the scratchpad if not already present, without
    /// moving the selection. Returns whether it was added. Used to add a
    /// freshly-generated synthesis note.
    pub fn add_path_to_scratchpad(&mut self, path: String) -> bool {
        if self.scratchpad.contains(&path) {
            return false;
        }
        self.scratchpad.push(path);
        self.sync_scratchpad_state();
        true
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

    /// A read-only snapshot of the rename modal, for rendering.
    pub fn rename_prompt(&self) -> Option<RenamePrompt> {
        self.renaming.as_ref().map(|state| RenamePrompt {
            prefix: state.prefix.clone(),
            slug: state.slug.clone(),
        })
    }

    /// Open the rename modal for the active note, prefilled with its current slug.
    /// Only timestamped Zettel notes are renameable (issue #1).
    fn start_rename(&mut self) {
        let Some(path) = self.active_note().map(|n| n.path.clone()) else {
            return;
        };
        if !rename::is_renameable(&path) {
            self.set_status("Only timestamped notes (YYYYMMDDHHMM-…) can be renamed.");
            return;
        }
        let prefix = Path::new(&path)
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|stem| rename::split_zettel(stem).map(|(p, _)| p.to_string()))
            .unwrap_or_default();
        let slug = rename::current_slug(&path).unwrap_or_default();
        self.renaming = Some(RenameState {
            path: PathBuf::from(path),
            prefix,
            slug,
        });
    }

    /// Handle a key while the rename modal is open.
    fn handle_rename_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => self.renaming = None,
            KeyCode::Backspace => {
                if let Some(state) = self.renaming.as_mut() {
                    state.slug.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(state) = self.renaming.as_mut() {
                    state.slug.push(c);
                }
            }
            KeyCode::Enter => {
                let Some(state) = self.renaming.take() else {
                    return Action::None;
                };
                let new_slug = state.slug.trim().to_string();
                if new_slug.is_empty() {
                    self.set_status("Rename cancelled — name was empty.");
                    return Action::None;
                }
                return Action::RenameNote {
                    path: state.path,
                    new_slug,
                };
            }
            _ => {}
        }
        Action::None
    }

    /// After a successful rename, retarget the selection and scratchpad from the
    /// old path to the new one so the view follows the renamed note.
    pub fn note_renamed(&mut self, old_path: &str, new_path: &str) {
        for entry in self.scratchpad.iter_mut() {
            if entry == old_path {
                *entry = new_path.to_string();
            }
        }
        if self.related_basis.as_deref() == Some(old_path) {
            self.related_basis = Some(new_path.to_string());
        }
    }

    /// Install the resolved LLM provider selection and per-provider models
    /// (called once at startup, after reading CLI + DB config).
    pub fn set_llm_config(&mut self, provider: ProviderKind, models: [String; 3], effort: Effort) {
        self.llm_provider_idx = ProviderKind::ALL
            .iter()
            .position(|&k| k == provider)
            .unwrap_or(0);
        self.llm_models = models;
        self.llm_effort = effort;
    }

    /// The active provider, its model, and the effort level, for a synthesis run.
    pub fn llm_selection(&self) -> (ProviderKind, String, Effort) {
        (
            ProviderKind::ALL[self.llm_provider_idx],
            self.llm_models[self.llm_provider_idx].clone(),
            self.llm_effort,
        )
    }

    /// The full config to persist: active provider, every provider's model, and
    /// the effort level.
    pub fn llm_config_for_persist(&self) -> (ProviderKind, [(ProviderKind, String); 3], Effort) {
        let models = [
            (ProviderKind::ALL[0], self.llm_models[0].clone()),
            (ProviderKind::ALL[1], self.llm_models[1].clone()),
            (ProviderKind::ALL[2], self.llm_models[2].clone()),
        ];
        (
            ProviderKind::ALL[self.llm_provider_idx],
            models,
            self.llm_effort,
        )
    }

    /// A read-only snapshot of the config modal, for rendering.
    pub fn config_view(&self) -> Option<ConfigView> {
        let provider = ProviderKind::ALL[self.llm_provider_idx];
        self.config_modal.as_ref().map(|m| ConfigView {
            provider_label: provider.label(),
            model: m
                .editing_model
                .clone()
                .unwrap_or_else(|| self.llm_models[self.llm_provider_idx].clone()),
            editing_model: m.editing_model.is_some(),
            effort: provider.supports_effort().then(|| self.llm_effort.label()),
            selected: m.selected,
            prompt_count: self.prompts.len(),
        })
    }

    /// Install the prompt library loaded from the DB.
    pub fn set_prompts(&mut self, prompts: Vec<llm::Prompt>) {
        self.prompts = prompts;
        if let Some(admin) = self.prompt_admin.as_mut() {
            if admin.editing.is_none() {
                admin.selected = admin.selected.min(self.prompts.len().saturating_sub(1));
            }
        }
    }

    /// Open the LLM config modal.
    fn open_config(&mut self) {
        self.config_modal = Some(ConfigModal {
            selected: CONFIG_ROW_PROVIDER,
            editing_model: None,
        });
    }

    /// Handle a key while the config modal is open.
    fn handle_config_key(&mut self, key: KeyEvent) -> Action {
        // Editing the model text field captures all input.
        if let Some(buf) = self
            .config_modal
            .as_ref()
            .and_then(|m| m.editing_model.clone())
        {
            match key.code {
                KeyCode::Esc => {
                    if let Some(m) = self.config_modal.as_mut() {
                        m.editing_model = None;
                    }
                }
                KeyCode::Enter => {
                    let model = buf.trim().to_string();
                    if let Some(m) = self.config_modal.as_mut() {
                        m.editing_model = None;
                    }
                    if !model.is_empty() {
                        self.llm_models[self.llm_provider_idx] = model;
                        return Action::LlmConfigChanged;
                    }
                }
                KeyCode::Backspace => {
                    if let Some(m) = self.config_modal.as_mut() {
                        if let Some(b) = m.editing_model.as_mut() {
                            b.pop();
                        }
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(m) = self.config_modal.as_mut() {
                        if let Some(b) = m.editing_model.as_mut() {
                            b.push(c);
                        }
                    }
                }
                _ => {}
            }
            return Action::None;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.config_modal = None,
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(m) = self.config_modal.as_mut() {
                    m.selected = (m.selected + CONFIG_ROWS - 1) % CONFIG_ROWS;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(m) = self.config_modal.as_mut() {
                    m.selected = (m.selected + 1) % CONFIG_ROWS;
                }
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l') => {
                let selected = self.config_modal.as_ref().map(|m| m.selected);
                let forward = matches!(key.code, KeyCode::Right | KeyCode::Char('l'));
                if selected == Some(CONFIG_ROW_PROVIDER) {
                    self.llm_provider_idx = if forward {
                        (self.llm_provider_idx + 1) % ProviderKind::ALL.len()
                    } else {
                        (self.llm_provider_idx + ProviderKind::ALL.len() - 1)
                            % ProviderKind::ALL.len()
                    };
                    return Action::LlmConfigChanged;
                }
                if selected == Some(CONFIG_ROW_EFFORT)
                    && ProviderKind::ALL[self.llm_provider_idx].supports_effort()
                {
                    let idx = Effort::ALL
                        .iter()
                        .position(|&e| e == self.llm_effort)
                        .unwrap_or(0);
                    let len = Effort::ALL.len();
                    let next = if forward {
                        (idx + 1) % len
                    } else {
                        (idx + len - 1) % len
                    };
                    self.llm_effort = Effort::ALL[next];
                    return Action::LlmConfigChanged;
                }
            }
            KeyCode::Enter => {
                let selected = self.config_modal.as_ref().map(|m| m.selected);
                match selected {
                    Some(CONFIG_ROW_MODEL) => {
                        let current = self.llm_models[self.llm_provider_idx].clone();
                        if let Some(m) = self.config_modal.as_mut() {
                            m.editing_model = Some(current);
                        }
                    }
                    Some(CONFIG_ROW_PROMPTS) => {
                        // Hand off to the prompt-library manager, closing the
                        // config modal so only one overlay is active.
                        self.config_modal = None;
                        self.prompt_admin = Some(PromptAdmin {
                            selected: 0,
                            editing: None,
                        });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Action::None
    }

    /// A read-only snapshot of the prompt picker, for rendering.
    pub fn prompt_picker_view(&self) -> Option<PromptPickerView> {
        self.prompt_picker
            .as_ref()
            .map(|picker| match &picker.custom {
                Some(text) => PromptPickerView::Custom { text: text.clone() },
                None => {
                    let mut labels: Vec<String> =
                        self.prompts.iter().map(|p| p.label.clone()).collect();
                    labels.push("Custom…".to_string());
                    PromptPickerView::List {
                        labels,
                        selected: picker.selected,
                    }
                }
            })
    }

    /// Open the prompt picker for LLM synthesis over the scratchpad.
    fn start_synthesis(&mut self) {
        if self.scratchpad.is_empty() {
            self.set_status("Add notes to the scratchpad first.");
            return;
        }
        self.prompt_picker = Some(PromptPicker {
            selected: 0,
            custom: None,
        });
    }

    /// Handle a key while the prompt picker is open.
    fn handle_prompt_key(&mut self, key: KeyEvent) -> Action {
        let in_custom = self
            .prompt_picker
            .as_ref()
            .is_some_and(|p| p.custom.is_some());

        if in_custom {
            match key.code {
                KeyCode::Esc => {
                    if let Some(p) = self.prompt_picker.as_mut() {
                        p.custom = None;
                    }
                }
                KeyCode::Enter => {
                    let text = self
                        .prompt_picker
                        .as_ref()
                        .and_then(|p| p.custom.clone())
                        .unwrap_or_default();
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        return self.confirm_synthesis(&text);
                    }
                }
                KeyCode::Backspace => {
                    if let Some(t) = self.prompt_picker.as_mut().and_then(|p| p.custom.as_mut()) {
                        t.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(t) = self.prompt_picker.as_mut().and_then(|p| p.custom.as_mut()) {
                        t.push(c);
                    }
                }
                _ => {}
            }
            return Action::None;
        }

        let count = self.prompts.len();
        match key.code {
            KeyCode::Esc => self.prompt_picker = None,
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(p) = self.prompt_picker.as_mut() {
                    p.selected = (p.selected + count) % (count + 1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(p) = self.prompt_picker.as_mut() {
                    p.selected = (p.selected + 1) % (count + 1);
                }
            }
            KeyCode::Enter => {
                let selected = self.prompt_picker.as_ref().map(|p| p.selected).unwrap_or(0);
                if selected >= count {
                    if let Some(p) = self.prompt_picker.as_mut() {
                        p.custom = Some(String::new());
                    }
                } else {
                    let text = self.prompts[selected].text.clone();
                    return self.confirm_synthesis(&text);
                }
            }
            KeyCode::Char(c @ '1'..='9') => {
                let idx = c as usize - '1' as usize;
                if idx < count {
                    let text = self.prompts[idx].text.clone();
                    return self.confirm_synthesis(&text);
                }
            }
            _ => {}
        }
        Action::None
    }

    /// Close the picker and request generation over the scratchpad notes.
    fn confirm_synthesis(&mut self, prompt_text: &str) -> Action {
        self.prompt_picker = None;
        let sources: Vec<(PathBuf, String)> = self
            .scratchpad
            .iter()
            .filter_map(|path| {
                self.notes
                    .iter()
                    .find(|n| &n.path == path)
                    .map(|n| (PathBuf::from(&n.path), n.title.clone()))
            })
            .collect();
        if sources.is_empty() {
            self.set_status("Scratchpad is empty.");
            return Action::None;
        }
        Action::GenerateSynthesis(SynthesisRequest {
            prompt_text: prompt_text.to_string(),
            sources,
        })
    }

    /// A read-only snapshot of the prompt manager, for rendering.
    pub fn prompt_admin_view(&self) -> Option<PromptAdminView> {
        self.prompt_admin
            .as_ref()
            .map(|admin| match &admin.editing {
                Some(edit) => PromptAdminView::Edit {
                    adding: edit.id.is_none(),
                    label: edit.label.clone(),
                    text: edit.text.clone(),
                    editing_label: edit.field == PromptField::Label,
                },
                None => PromptAdminView::List {
                    prompts: self.prompts.iter().map(|p| p.label.clone()).collect(),
                    selected: admin.selected,
                },
            })
    }

    /// Handle a key while the prompt-library manager is open.
    fn handle_prompt_admin_key(&mut self, key: KeyEvent) -> Action {
        // Editing/adding a prompt captures all input.
        if self
            .prompt_admin
            .as_ref()
            .is_some_and(|a| a.editing.is_some())
        {
            return self.handle_prompt_edit_key(key);
        }

        let count = self.prompts.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.prompt_admin = None,
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(a) = self.prompt_admin.as_mut() {
                    if count > 0 {
                        a.selected = (a.selected + count - 1) % count;
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(a) = self.prompt_admin.as_mut() {
                    if count > 0 {
                        a.selected = (a.selected + 1) % count;
                    }
                }
            }
            KeyCode::Char('a') => {
                if let Some(a) = self.prompt_admin.as_mut() {
                    a.editing = Some(PromptEdit {
                        id: None,
                        field: PromptField::Label,
                        label: String::new(),
                        text: String::new(),
                    });
                }
            }
            KeyCode::Char('e') | KeyCode::Enter => {
                let sel = self.prompt_admin.as_ref().map(|a| a.selected).unwrap_or(0);
                if let Some(prompt) = self.prompts.get(sel).cloned() {
                    if let Some(a) = self.prompt_admin.as_mut() {
                        a.editing = Some(PromptEdit {
                            id: Some(prompt.id),
                            field: PromptField::Label,
                            label: prompt.label,
                            text: prompt.text,
                        });
                    }
                }
            }
            KeyCode::Char('d') => {
                let sel = self.prompt_admin.as_ref().map(|a| a.selected).unwrap_or(0);
                if let Some(prompt) = self.prompts.get(sel) {
                    return Action::PromptDelete { id: prompt.id };
                }
            }
            // Shift+J / Shift+K reorder the selected prompt down / up.
            KeyCode::Char('J') => {
                let sel = self.prompt_admin.as_ref().map(|a| a.selected).unwrap_or(0);
                if sel + 1 < count {
                    if let Some(a) = self.prompt_admin.as_mut() {
                        a.selected = sel + 1;
                    }
                    return Action::PromptReorder {
                        id_a: self.prompts[sel].id,
                        id_b: self.prompts[sel + 1].id,
                    };
                }
            }
            KeyCode::Char('K') => {
                let sel = self.prompt_admin.as_ref().map(|a| a.selected).unwrap_or(0);
                if sel > 0 && sel < count {
                    if let Some(a) = self.prompt_admin.as_mut() {
                        a.selected = sel - 1;
                    }
                    return Action::PromptReorder {
                        id_a: self.prompts[sel].id,
                        id_b: self.prompts[sel - 1].id,
                    };
                }
            }
            _ => {}
        }
        Action::None
    }

    /// Handle a key while adding/editing a single prompt.
    fn handle_prompt_edit_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                if let Some(a) = self.prompt_admin.as_mut() {
                    a.editing = None;
                }
            }
            KeyCode::Tab => {
                if let Some(edit) = self.prompt_admin.as_mut().and_then(|a| a.editing.as_mut()) {
                    edit.field = match edit.field {
                        PromptField::Label => PromptField::Text,
                        PromptField::Text => PromptField::Label,
                    };
                }
            }
            KeyCode::Enter => {
                // Save. Both fields must be non-empty.
                if let Some(edit) = self.prompt_admin.as_ref().and_then(|a| a.editing.as_ref()) {
                    let label = edit.label.trim().to_string();
                    let text = edit.text.trim().to_string();
                    let id = edit.id;
                    if label.is_empty() || text.is_empty() {
                        self.set_status("A prompt needs both a label and text.");
                        return Action::None;
                    }
                    if let Some(a) = self.prompt_admin.as_mut() {
                        a.editing = None;
                    }
                    return match id {
                        Some(id) => Action::PromptUpdate { id, label, text },
                        None => Action::PromptAdd { label, text },
                    };
                }
            }
            KeyCode::Backspace => {
                if let Some(edit) = self.prompt_admin.as_mut().and_then(|a| a.editing.as_mut()) {
                    match edit.field {
                        PromptField::Label => edit.label.pop(),
                        PromptField::Text => edit.text.pop(),
                    };
                }
            }
            KeyCode::Char(c) => {
                if let Some(edit) = self.prompt_admin.as_mut().and_then(|a| a.editing.as_mut()) {
                    match edit.field {
                        PromptField::Label => edit.label.push(c),
                        PromptField::Text => edit.text.push(c),
                    }
                }
            }
            _ => {}
        }
        Action::None
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
            // Point the related list at whatever the newly-focused pane makes
            // the note of interest (Related keeps its current list).
            self.refresh_related();
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
        // Preview and relate to the highlighted scratchpad note.
        self.refresh_related();
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
        if let Some(target) = self
            .related
            .get(self.related_selected)
            .map(|r| r.path.clone())
        {
            self.jump_to_path(&target);
        }
    }

    /// Focus the notes list on `path` if it is indexed (e.g. after a rename).
    pub fn focus_path(&mut self, path: &str) {
        self.focus = Focus::Notes;
        self.jump_to_path(path);
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
        if let Some(pos) = self
            .filtered
            .iter()
            .position(|&i| self.notes[i].path == path)
        {
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

    /// Called whenever the *anchor* note changes: refresh the related list and
    /// re-render the preview.
    fn select_note_changed(&mut self) {
        self.refresh_related();
        self.rebuild_preview();
    }

    /// The note the related list should be computed from: the highlighted
    /// scratchpad note while pane 4 has focus, else the anchor. While the
    /// Related pane has focus the basis is left as-is, so browsing it doesn't
    /// pull the list out from under you.
    fn note_of_interest(&self) -> Option<String> {
        match self.focus {
            Focus::Scratchpad => self.scratchpad.get(self.scratchpad_selected).cloned(),
            Focus::Related => self.related_basis.clone(),
            _ => self.anchor_note().map(|n| n.path.clone()),
        }
    }

    /// Point the related list at the current note of interest and recompute it.
    fn refresh_related(&mut self) {
        self.related_basis = self.note_of_interest();
        self.related_selected = 0;
        self.recompute_related();
        self.sync_related_state();
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
        self.related = match &self.related_basis {
            Some(path) => {
                let cheap = self.related_index.related(path, 12);
                let semantic = self
                    .semantic
                    .as_ref()
                    .map(|s| s.similar(path, 12))
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
