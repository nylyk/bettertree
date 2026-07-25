use std::io::stdout;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::DefaultTerminal;

/// Runs the editor in this terminal and restores the TUI afterwards.
///
/// The alternate screen is deliberately *not* left first: the editor switches to it itself, so
/// staying put means the shell never becomes visible in between. Callers must hand over stdin
/// first, see `Events::suspend`.
pub fn open(terminal: &mut DefaultTerminal, path: &Path, configured: &str) -> Result<()> {
    let command = resolve(configured);
    let mut parts = command.split_whitespace();

    let Some(program) = parts.next() else {
        bail!("no editor configured and $EDITOR is unset");
    };
    let args: Vec<&str> = parts.collect();

    disable_raw_mode()?;
    execute!(stdout(), Show)?;

    let status = Command::new(program).args(args).arg(path).status();

    // An editor that used the alternate screen has just left it, so claim it back either way.
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, Hide)?;
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
