use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::ValueEnum;

use crate::cli::Cli;

/// Name of the per-vault state directory. Excluded from the vault scan.
pub const STATE_DIR: &str = ".constellate";

/// How a note reference is formatted when yanked to the clipboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum RefFormat {
    /// Path relative to the vault root, e.g. `subdir/note.md`.
    #[default]
    Relative,
    /// Absolute filesystem path.
    Absolute,
    /// Obsidian-style `[[Title]]` wikilink.
    Wikilink,
}

/// Resolved runtime configuration derived from CLI arguments and the environment.
pub struct Config {
    /// Absolute, canonicalized path to the vault root.
    pub vault: PathBuf,
    /// Path to the SQLite index database.
    pub db_path: PathBuf,
    /// Path to the app log file (background-worker events).
    pub log_path: PathBuf,
    /// Default format for clipboard note references.
    pub ref_format: RefFormat,
    /// Whether semantic embeddings are enabled.
    pub embed_enabled: bool,
    /// Ollama embedding model.
    pub embed_model: String,
    /// Base URL of the Ollama server.
    pub ollama_url: String,
}

impl Config {
    pub fn from_cli(cli: Cli) -> Result<Self> {
        let raw_vault = cli
            .vault
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let vault = raw_vault
            .canonicalize()
            .with_context(|| format!("vault path does not exist: {}", raw_vault.display()))?;

        anyhow::ensure!(
            vault.is_dir(),
            "vault path is not a directory: {}",
            vault.display()
        );

        let db_path = cli
            .db
            .unwrap_or_else(|| vault.join(STATE_DIR).join("index.db"));

        // Keep the log alongside the database so it lands in a directory the
        // store already creates.
        let log_path = db_path.with_file_name("constellate.log");

        Ok(Config {
            vault,
            db_path,
            log_path,
            ref_format: cli.ref_format,
            embed_enabled: !cli.no_embed,
            embed_model: cli.embed_model,
            ollama_url: cli.ollama_url,
        })
    }
}
