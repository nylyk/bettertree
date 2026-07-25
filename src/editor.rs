use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use ratatui::DefaultTerminal;

/// Leaves the TUI, runs the editor in the same terminal, then restores the TUI.
pub fn open(terminal: &mut DefaultTerminal, path: &Path, configured: &str) -> Result<()> {
    let command = resolve(configured);
    let mut parts = command.split_whitespace();

    let Some(program) = parts.next() else {
        bail!("no editor configured and $EDITOR is unset");
    };
    let args: Vec<&str> = parts.collect();

    ratatui::restore();
    let status = Command::new(program).args(args).arg(path).status();
    *terminal = ratatui::init();
    terminal.clear()?;

    let status = status.with_context(|| format!("failed to run {program}"))?;
    if !status.success() {
        bail!("{program} exited with {status}");
    }

    Ok(())
}

fn resolve(configured: &str) -> String {
    if !configured.trim().is_empty() {
        return configured.to_owned();
    }

    std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_owned())
}
