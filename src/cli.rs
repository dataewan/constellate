use std::path::PathBuf;

use clap::Parser;

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
}
