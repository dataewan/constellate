use std::path::PathBuf;

use clap::Parser;

use crate::config::{EmbedBackend, RefFormat};

/// Terminal-native knowledge management & discovery for a Markdown vault.
#[derive(Parser, Debug)]
#[command(name = "constellate", version, about)]
pub struct Cli {
    /// Path to the Markdown vault to index and browse. Defaults to the current directory.
    #[arg(short, long, value_name = "DIR")]
    pub vault: Option<PathBuf>,

    /// Override the index database location. Defaults to <vault>/.constellate/index.db.
    #[arg(long, value_name = "FILE")]
    pub db: Option<PathBuf>,

    /// Format used when yanking a note reference to the clipboard.
    #[arg(long, value_enum, default_value = "relative")]
    pub ref_format: RefFormat,

    /// Disable semantic embeddings (skip the backend; use only cheap relatedness).
    #[arg(long)]
    pub no_embed: bool,

    /// Embedding backend. `fastembed` requires building with `--features fastembed`.
    #[arg(long, value_enum, default_value = "ollama")]
    pub embed_backend: EmbedBackend,

    /// Ollama embedding model (used when `--embed-backend ollama`).
    #[arg(long, default_value = "nomic-embed-text")]
    pub embed_model: String,

    /// Base URL of the Ollama server (used when `--embed-backend ollama`).
    #[arg(long, default_value = "http://localhost:11434")]
    pub ollama_url: String,
}
