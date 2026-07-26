use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::ValueEnum;

use crate::cli::Cli;
use crate::embed::Backend;

/// Name of the per-vault state directory. Excluded from the vault scan.
pub const STATE_DIR: &str = ".constellate";

/// Embedding backend chosen on the command line. Always parseable; selecting
/// `Fastembed` without the `fastembed` feature is rejected at startup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum EmbedBackend {
    /// Local Ollama server.
    #[default]
    Ollama,
    /// In-process ONNX embeddings (requires `--features fastembed`).
    Fastembed,
}

/// How a note reference is formatted when yanked to the clipboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum RefFormat {
    /// Markdown link to the note, e.g. `[Title](subdir/note.md)`.
    #[default]
    Markdown,
    /// Path relative to the vault root, e.g. `subdir/note.md`.
    Relative,
    /// Absolute filesystem path.
    Absolute,
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
    /// The embedding backend, or `None` when embeddings are disabled.
    pub embed_backend: Option<Backend>,
    /// Base URL of the Ollama server (for LLM synthesis).
    pub ollama_url: String,
    /// Ollama chat model for scratchpad LLM synthesis.
    pub llm_model: String,
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

        let embed_backend = if cli.no_embed {
            None
        } else {
            Some(resolve_backend(
                cli.embed_backend,
                cli.ollama_url.clone(),
                cli.embed_model,
            )?)
        };

        Ok(Config {
            vault,
            db_path,
            log_path,
            ref_format: cli.ref_format,
            embed_backend,
            ollama_url: cli.ollama_url,
            llm_model: cli.llm_model,
        })
    }
}

fn resolve_backend(backend: EmbedBackend, ollama_url: String, model: String) -> Result<Backend> {
    match backend {
        EmbedBackend::Ollama => Ok(Backend::Ollama {
            url: ollama_url,
            model,
        }),
        EmbedBackend::Fastembed => {
            #[cfg(feature = "fastembed")]
            {
                Ok(Backend::FastEmbed)
            }
            #[cfg(not(feature = "fastembed"))]
            {
                anyhow::bail!(
                    "--embed-backend fastembed requires building with `--features fastembed`"
                )
            }
        }
    }
}
