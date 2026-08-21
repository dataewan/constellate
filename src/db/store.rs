use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::llm::{Effort, ProviderKind};
use crate::vault::ParsedNote;

/// A note loaded for browsing, searching, preview, and relatedness.
#[derive(Debug, Clone)]
pub struct NoteRow {
    pub path: String,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub links: Vec<String>,
}

/// SQLite-backed index. Owns the connection; used only from the main thread.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) the index at `db_path` and run migrations.
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating state dir {}", parent.display()))?;
        }
        let conn = Connection::open(db_path)
            .with_context(|| format!("opening index db {}", db_path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS notes (
                path         TEXT PRIMARY KEY,
                title        TEXT NOT NULL,
                frontmatter  TEXT,
                content      TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                updated_at   INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chunks (
                id           INTEGER PRIMARY KEY,
                note_path    TEXT NOT NULL REFERENCES notes(path) ON DELETE CASCADE,
                header       TEXT NOT NULL,
                content      TEXT NOT NULL,
                content_hash TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_chunks_note ON chunks(note_path);

            CREATE TABLE IF NOT EXISTS links (
                note_path TEXT NOT NULL REFERENCES notes(path) ON DELETE CASCADE,
                target    TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_links_note ON links(note_path);
            CREATE INDEX IF NOT EXISTS idx_links_target ON links(target);

            CREATE TABLE IF NOT EXISTS tags (
                note_path TEXT NOT NULL REFERENCES notes(path) ON DELETE CASCADE,
                tag       TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_tags_note ON tags(note_path);
            CREATE INDEX IF NOT EXISTS idx_tags_tag ON tags(tag);

            CREATE TABLE IF NOT EXISTS embeddings (
                chunk_id INTEGER PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE,
                dim      INTEGER NOT NULL,
                vector   BLOB NOT NULL
            );

            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- The scratchpad: an ordered working set of note paths. Standalone
            -- (no FK to notes) so re-indexing a note — which deletes and
            -- re-inserts its row — does not drop it from the scratchpad.
            CREATE TABLE IF NOT EXISTS scratchpad (
                note_path TEXT PRIMARY KEY,
                position  INTEGER NOT NULL
            );
            "#,
        )?;
        Ok(())
    }

    /// The scratchpad note paths, in order.
    pub fn load_scratchpad(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT note_path FROM scratchpad ORDER BY position")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Replace the scratchpad with `paths`, preserving their order.
    pub fn save_scratchpad(&mut self, paths: &[String]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM scratchpad", [])?;
        for (i, path) in paths.iter().enumerate() {
            tx.execute(
                "INSERT INTO scratchpad (note_path, position) VALUES (?1, ?2)",
                params![path, i as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn meta_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get::<_, String>(0)
            })
            .ok())
    }

    fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// The persisted LLM provider selection, if the user has chosen one.
    pub fn llm_provider(&self) -> Result<Option<ProviderKind>> {
        Ok(self
            .meta_get("llm_provider")?
            .and_then(|s| ProviderKind::parse(&s)))
    }

    /// The persisted model for a given provider, if set (stored per-provider so
    /// switching backends remembers each one's model).
    pub fn llm_model(&self, kind: ProviderKind) -> Result<Option<String>> {
        self.meta_get(&format!("llm_model_{}", kind.as_str()))
    }

    /// Persist the active LLM provider.
    pub fn set_llm_provider(&self, kind: ProviderKind) -> Result<()> {
        self.meta_set("llm_provider", kind.as_str())
    }

    /// Persist the model for a given provider.
    pub fn set_llm_model(&self, kind: ProviderKind, model: &str) -> Result<()> {
        self.meta_set(&format!("llm_model_{}", kind.as_str()), model)
    }

    /// The persisted reasoning-effort level, if set. Applies to whichever
    /// hosted provider is active; ignored by Ollama.
    pub fn llm_effort(&self) -> Result<Option<Effort>> {
        Ok(self.meta_get("llm_effort")?.and_then(|s| Effort::parse(&s)))
    }

    /// Persist the reasoning-effort level.
    pub fn set_llm_effort(&self, effort: Effort) -> Result<()> {
        self.meta_set("llm_effort", effort.as_str())
    }

    /// Ensure stored embeddings match the active backend/model. If the model
    /// changed, discard all embeddings so they are recomputed with the new one.
    pub fn reconcile_embedding_backend(&mut self, model: &str) -> Result<()> {
        if self.meta_get("embed_model")?.as_deref() != Some(model) {
            self.conn.execute("DELETE FROM embeddings", [])?;
            self.meta_set("embed_model", model)?;
        }
        Ok(())
    }

    /// Chunks that still lack an embedding, as `(chunk_id, text)` pairs.
    pub fn chunks_without_embeddings(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.header, c.content
             FROM chunks c
             LEFT JOIN embeddings e ON e.chunk_id = c.id
             WHERE e.chunk_id IS NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let header: String = row.get(1)?;
            let content: String = row.get(2)?;
            Ok((id, format!("{header}\n{content}").trim().to_string()))
        })?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|(_, text)| !text.is_empty())
            .collect())
    }

    /// Persist an embedding vector for a chunk.
    pub fn store_embedding(&mut self, chunk_id: i64, vector: &[f32]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO embeddings (chunk_id, dim, vector) VALUES (?1, ?2, ?3)
             ON CONFLICT(chunk_id) DO UPDATE SET dim = excluded.dim, vector = excluded.vector",
            params![chunk_id, vector.len() as i64, crate::embed::to_blob(vector)],
        )?;
        Ok(())
    }

    /// Every stored chunk embedding as `(note_path, vector)`.
    pub fn all_chunk_vectors(&self) -> Result<Vec<(String, Vec<f32>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.note_path, e.vector
             FROM embeddings e
             JOIN chunks c ON c.id = e.chunk_id",
        )?;
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((path, crate::embed::from_blob(&blob)))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The stored content hash for a note path, if indexed.
    pub fn stored_hash(&self, path: &str) -> Result<Option<String>> {
        let hash = self
            .conn
            .query_row(
                "SELECT content_hash FROM notes WHERE path = ?1",
                [path],
                |row| row.get::<_, String>(0),
            )
            .ok();
        Ok(hash)
    }

    /// Insert or replace a note and its chunks/links/tags in one transaction.
    pub fn upsert_note(&mut self, note: &ParsedNote, hash: &str) -> Result<()> {
        let key = note.path.to_string_lossy().to_string();
        let now = unix_now();
        let tx = self.conn.transaction()?;

        // ON DELETE CASCADE clears dependent rows when the note is replaced.
        tx.execute("DELETE FROM notes WHERE path = ?1", [&key])?;
        tx.execute(
            "INSERT INTO notes (path, title, frontmatter, content, content_hash, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![key, note.title, note.frontmatter, note.content, hash, now],
        )?;

        for chunk in &note.chunks {
            tx.execute(
                "INSERT INTO chunks (note_path, header, content, content_hash)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    key,
                    chunk.header,
                    chunk.content,
                    crate::vault::hash_content(&chunk.content)
                ],
            )?;
        }
        for target in &note.links {
            tx.execute(
                "INSERT INTO links (note_path, target) VALUES (?1, ?2)",
                params![key, target],
            )?;
        }
        for tag in &note.tags {
            tx.execute(
                "INSERT INTO tags (note_path, tag) VALUES (?1, ?2)",
                params![key, tag],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Remove a note and (via cascade) its dependent rows.
    pub fn delete_note(&mut self, path: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM notes WHERE path = ?1", [path])?;
        Ok(())
    }

    /// All indexed note paths.
    pub fn all_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT path FROM notes")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// All notes ordered by title, each with its tags and outgoing links,
    /// for the browse list and the related-notes engine.
    pub fn all_notes(&self) -> Result<Vec<NoteRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, title, content FROM notes ORDER BY title COLLATE NOCASE")?;
        let mut notes: Vec<NoteRow> = stmt
            .query_map([], |row| {
                Ok(NoteRow {
                    path: row.get(0)?,
                    title: row.get(1)?,
                    content: row.get(2)?,
                    tags: Vec::new(),
                    links: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let index: HashMap<String, usize> = notes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.path.clone(), i))
            .collect();

        let mut tag_stmt = self.conn.prepare("SELECT note_path, tag FROM tags")?;
        let tag_rows = tag_stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in tag_rows {
            let (path, tag) = row?;
            if let Some(&i) = index.get(&path) {
                notes[i].tags.push(tag);
            }
        }

        let mut link_stmt = self.conn.prepare("SELECT note_path, target FROM links")?;
        let link_rows = link_stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in link_rows {
            let (path, target) = row?;
            if let Some(&i) = index.get(&path) {
                notes[i].links.push(target);
            }
        }

        Ok(notes)
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault;
    use std::path::PathBuf;

    fn temp_db() -> PathBuf {
        let mut p = std::env::temp_dir();
        let unique = format!(
            "constellate-test-{}-{}.db",
            std::process::id(),
            unix_now_nanos()
        );
        p.push(unique);
        p
    }

    fn unix_now_nanos() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    #[test]
    fn upsert_delete_and_incremental_hash() {
        let db = temp_db();
        let mut store = Store::open(&db).unwrap();

        let note = vault::parse(
            &PathBuf::from("/vault/a.md"),
            "# A\nhello [[B]] #tag".to_string(),
        );
        let hash = vault::hash_content("# A\nhello [[B]] #tag");

        store.upsert_note(&note, &hash).unwrap();
        assert_eq!(store.all_notes().unwrap().len(), 1);
        assert_eq!(
            store.stored_hash("/vault/a.md").unwrap().as_deref(),
            Some(hash.as_str())
        );

        // Re-writing replaces cleanly (no duplicate chunks/links/tags).
        store.upsert_note(&note, &hash).unwrap();
        assert_eq!(store.all_notes().unwrap().len(), 1);

        store.delete_note("/vault/a.md").unwrap();
        assert!(store.all_notes().unwrap().is_empty());
        assert!(store.stored_hash("/vault/a.md").unwrap().is_none());

        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn llm_config_roundtrip() {
        let db = temp_db();
        let store = Store::open(&db).unwrap();

        // Unset by default.
        assert_eq!(store.llm_provider().unwrap(), None);
        assert_eq!(store.llm_model(ProviderKind::Claude).unwrap(), None);

        store.set_llm_provider(ProviderKind::Claude).unwrap();
        store
            .set_llm_model(ProviderKind::Claude, "claude-sonnet-5")
            .unwrap();
        store.set_llm_model(ProviderKind::Ollama, "llama3").unwrap();

        assert_eq!(store.llm_provider().unwrap(), Some(ProviderKind::Claude));
        assert_eq!(
            store.llm_model(ProviderKind::Claude).unwrap().as_deref(),
            Some("claude-sonnet-5")
        );
        // Models are stored per-provider and don't collide.
        assert_eq!(
            store.llm_model(ProviderKind::Ollama).unwrap().as_deref(),
            Some("llama3")
        );
        assert_eq!(store.llm_model(ProviderKind::Gemini).unwrap(), None);

        // Effort is unset by default, then round-trips.
        assert_eq!(store.llm_effort().unwrap(), None);
        store.set_llm_effort(Effort::High).unwrap();
        assert_eq!(store.llm_effort().unwrap(), Some(Effort::High));

        let _ = std::fs::remove_file(&db);
    }
}
