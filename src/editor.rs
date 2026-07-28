use std::ffi::OsString;
use std::path::Path;

use anyhow::{Result, bail};

/// The command line that opens `path` in the configured editor.
pub fn command(configured: &str, path: &Path) -> Result<Vec<OsString>> {
    let mut argv: Vec<OsString> = resolve(configured)
        .split_whitespace()
        .map(OsString::from)
        .collect();

    if argv.is_empty() {
        bail!("no editor configured and $EDITOR is unset");
    }
    argv.push(path.into());

    Ok(argv)
}

fn resolve(configured: &str) -> String {
    if !configured.trim().is_empty() {
        return configured.to_owned();
    }

    std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_owned())
}
