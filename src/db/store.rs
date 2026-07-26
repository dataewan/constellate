use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::vault::ParsedNote;

/// A note row loaded for browsing, searching, and preview.
#[derive(Debug, Clone)]
pub struct NoteRow {
    pub path: String,
    pub title: String,
    pub content: String,
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

            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;
        Ok(())
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

    /// All notes ordered by title, for the browse list.
    pub fn all_notes(&self) -> Result<Vec<NoteRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, title, content FROM notes ORDER BY title COLLATE NOCASE")?;
        let rows = stmt.query_map([], |row| {
            Ok(NoteRow {
                path: row.get(0)?,
                title: row.get(1)?,
                content: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
        assert_eq!(store.stored_hash("/vault/a.md").unwrap().as_deref(), Some(hash.as_str()));

        // Re-writing replaces cleanly (no duplicate chunks/links/tags).
        store.upsert_note(&note, &hash).unwrap();
        assert_eq!(store.all_notes().unwrap().len(), 1);

        store.delete_note("/vault/a.md").unwrap();
        assert!(store.all_notes().unwrap().is_empty());
        assert!(store.stored_hash("/vault/a.md").unwrap().is_none());

        let _ = std::fs::remove_file(&db);
    }
}
