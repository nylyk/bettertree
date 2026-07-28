use std::ffi::OsStr;
use std::io::stdout;
use std::process::Command;

use anyhow::{Context, Result, bail};
use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::DefaultTerminal;

/// Runs a child in this terminal and restores the TUI afterwards.
///
/// The alternate screen is deliberately *not* left first: a full-screen child switches to it
/// itself, so staying put means the shell never becomes visible in between. Callers must hand
/// over stdin first, see `Events::suspend`.
pub fn run(terminal: &mut DefaultTerminal, argv: &[impl AsRef<OsStr>]) -> Result<()> {
    let Some((program, args)) = argv.split_first() else {
        bail!("nothing to run");
    };
    let name = program.as_ref().to_string_lossy().into_owned();

    disable_raw_mode()?;
    execute!(stdout(), Show)?;

    let status = Command::new(program).args(args).status();

    // A child that used the alternate screen has just left it, so claim it back either way.
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, Hide)?;
    terminal.clear()?;

    let status = status.with_context(|| format!("failed to run {name}"))?;
    if !status.success() {
        bail!("{name} exited with {status}");
    }

    Ok(())
}
