use anyhow::{Context, Result};

/// Copy text to the system clipboard.
///
/// Note: on some Linux/X11 setups clipboard ownership ends when the process
/// exits; constellate keeps running after a yank, so the reference stays
/// available for pasting. On macOS/Windows the value persists regardless.
pub fn copy(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("opening system clipboard")?;
    clipboard
        .set_text(text.to_string())
        .context("writing to system clipboard")?;
    Ok(())
}
