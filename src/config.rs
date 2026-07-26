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
    /// Default format for clipboard note references.
    pub ref_format: RefFormat,
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

        Ok(Config {
            vault,
            db_path,
            ref_format: cli.ref_format,
        })
    }
}
