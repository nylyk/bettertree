mod app;
mod clipboard;
mod command_line;
mod commands;
mod config;
mod editor;
mod events;
mod git;
mod search;
mod state;
mod tree;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};

const HELP: &str = "\
bt: an interactive file tree

Usage: bt [PATH]

Arguments:
  [PATH]  Directory to open (default: the current directory)

Options:
  -h, --help     Print this help
  -V, --version  Print the version
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("bt: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let Some(root) = parse_args()? else {
        return Ok(());
    };

    let config = config::Config::load()?;

    let mut terminal = ratatui::init();
    let result = app::App::new(root, config).run(&mut terminal);
    ratatui::restore();

    result
}

/// Returns the directory to open, or `None` when a flag was handled and we should just exit.
fn parse_args() -> Result<Option<PathBuf>> {
    let mut path = None;

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("bt {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            _ if arg.starts_with('-') => bail!("unknown option: {arg}"),
            _ if path.is_some() => bail!("expected at most one path"),
            _ => path = Some(PathBuf::from(arg)),
        }
    }

    let path = path.unwrap_or_else(|| PathBuf::from("."));
    let root = path
        .canonicalize()
        .with_context(|| format!("cannot open {}", path.display()))?;

    if !root.is_dir() {
        bail!("not a directory: {}", root.display());
    }

    Ok(Some(root))
}
