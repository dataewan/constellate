//! A small single-line text input with readline/emacs-style editing.
//!
//! Every modal text field (search, model name, rename slug, custom prompt,
//! prompt label/text) shares this so the same key bindings work everywhere:
//! cursor movement (←/→, Ctrl-b/Ctrl-f, Home/End, Ctrl-a/Ctrl-e), word/line
//! kills (Ctrl-w, Ctrl-u, Ctrl-k), word movement (Alt-b/Alt-f), and character
//! deletion (Backspace, Delete/Ctrl-d). Enter/Tab/Esc are left to the caller.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A single-line editable buffer with a cursor. The cursor is a **character**
/// index in `0..=chars.len()`, so multi-byte characters are handled correctly.
#[derive(Clone, Default)]
pub struct TextInput {
    chars: Vec<char>,
    cursor: usize,
}

/// A render-ready snapshot: the text and the cursor's character offset.
pub struct InputView {
    pub text: String,
    pub cursor: usize,
}

impl TextInput {
    /// An empty input.
    pub fn new() -> Self {
        Self::default()
    }

    /// An input pre-filled with `text`, cursor at the end.
    pub fn with_text(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let cursor = chars.len();
        TextInput { chars, cursor }
    }

    /// The current text.
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// A snapshot for rendering.
    pub fn view(&self) -> InputView {
        InputView {
            text: self.text(),
            cursor: self.cursor,
        }
    }

    /// Handle a key. Returns `true` if the text content changed (cursor-only
    /// moves return `false`), so callers like search can re-filter on edits.
    /// Enter/Tab/Esc are ignored here — the caller handles those.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            // --- editing ---
            KeyCode::Char('w') if ctrl => return self.delete_word_before(),
            KeyCode::Char('u') if ctrl => return self.delete_to_start(),
            KeyCode::Char('k') if ctrl => return self.delete_to_end(),
            KeyCode::Char('d') if ctrl => return self.delete_at(),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.chars.len(),
            KeyCode::Char('b') if ctrl => self.move_left(),
            KeyCode::Char('f') if ctrl => self.move_right(),
            KeyCode::Char('b') if alt => self.move_word_left(),
            KeyCode::Char('f') if alt => self.move_word_right(),
            KeyCode::Char(c) if !ctrl && !alt => {
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
                return true;
            }
            KeyCode::Backspace => return self.delete_before(),
            KeyCode::Delete => return self.delete_at(),
            // --- cursor movement ---
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.chars.len(),
            _ => {}
        }
        false
    }

    fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_right(&mut self) {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
        }
    }

    fn move_word_left(&mut self) {
        self.cursor = self.word_start_before(self.cursor);
    }

    fn move_word_right(&mut self) {
        self.cursor = self.word_end_after(self.cursor);
    }

    fn delete_before(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        self.chars.remove(self.cursor);
        true
    }

    fn delete_at(&mut self) -> bool {
        if self.cursor >= self.chars.len() {
            return false;
        }
        self.chars.remove(self.cursor);
        true
    }

    fn delete_word_before(&mut self) -> bool {
        let start = self.word_start_before(self.cursor);
        if start == self.cursor {
            return false;
        }
        self.chars.drain(start..self.cursor);
        self.cursor = start;
        true
    }

    fn delete_to_start(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.chars.drain(0..self.cursor);
        self.cursor = 0;
        true
    }

    fn delete_to_end(&mut self) -> bool {
        if self.cursor >= self.chars.len() {
            return false;
        }
        self.chars.truncate(self.cursor);
        true
    }

    /// The index of the start of the word at or before `from` (skipping any
    /// whitespace immediately before the cursor first).
    fn word_start_before(&self, from: usize) -> usize {
        let mut i = from;
        while i > 0 && self.chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    /// The index just past the end of the word at or after `from`.
    fn word_end_after(&self, from: usize) -> usize {
        let len = self.chars.len();
        let mut i = from;
        while i < len && self.chars[i].is_whitespace() {
            i += 1;
        }
        while i < len && !self.chars[i].is_whitespace() {
            i += 1;
        }
        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEvent;

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn plain(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn typed(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn insert_at_cursor_after_moving_left() {
        let mut input = TextInput::with_text("ac");
        input.handle(plain(KeyCode::Left));
        assert!(input.handle(typed('b')));
        assert_eq!(input.text(), "abc");
    }

    #[test]
    fn home_end_and_ctrl_variants() {
        let mut input = TextInput::with_text("hello");
        input.handle(ctrl('a'));
        assert_eq!(input.view().cursor, 0);
        input.handle(ctrl('e'));
        assert_eq!(input.view().cursor, 5);
    }

    #[test]
    fn ctrl_w_deletes_previous_word() {
        let mut input = TextInput::with_text("foo bar baz");
        assert!(input.handle(ctrl('w')));
        assert_eq!(input.text(), "foo bar ");
        // Trailing space then the word both go on the next kill.
        input.handle(ctrl('w'));
        assert_eq!(input.text(), "foo ");
    }

    #[test]
    fn ctrl_u_and_ctrl_k_kill_line() {
        let mut input = TextInput::with_text("hello world");
        input.handle(ctrl('a'));
        input.handle(plain(KeyCode::Right)); // cursor after 'h'
        assert!(input.handle(ctrl('k')));
        assert_eq!(input.text(), "h");

        let mut input = TextInput::with_text("hello");
        input.handle(ctrl('e'));
        assert!(input.handle(ctrl('u')));
        assert_eq!(input.text(), "");
    }

    #[test]
    fn delete_at_cursor() {
        let mut input = TextInput::with_text("abc");
        input.handle(ctrl('a'));
        assert!(input.handle(ctrl('d')));
        assert_eq!(input.text(), "bc");
    }

    #[test]
    fn cursor_only_moves_report_no_change() {
        let mut input = TextInput::with_text("abc");
        assert!(!input.handle(plain(KeyCode::Left)));
        assert!(!input.handle(ctrl('a')));
    }

    #[test]
    fn word_movement() {
        let mut input = TextInput::with_text("foo bar");
        input.handle(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT));
        assert_eq!(input.view().cursor, 4); // start of "bar"
    }
}
