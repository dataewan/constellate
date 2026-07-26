use std::path::PathBuf;

use clap::Parser;

use crate::config::RefFormat;

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
}
