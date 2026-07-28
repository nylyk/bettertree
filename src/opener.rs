use std::ffi::OsString;
use std::path::Path;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// The desktop's own opener, and the arguments that come before the path.
///
/// Windows has no launcher binary, only the `start` builtin of `cmd`, whose first argument is a
/// window title: it has to be there, or the path is taken for one.
#[cfg(target_os = "macos")]
const LAUNCHER: (&str, &[&str]) = ("open", &[]);
#[cfg(target_os = "windows")]
const LAUNCHER: (&str, &[&str]) = ("cmd", &["/C", "start", ""]);
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const LAUNCHER: (&str, &[&str]) = ("xdg-open", &[]);

/// Hands the path to the desktop's default application.
///
/// The child keeps running after bettertree exits and must never touch this terminal, so its
/// streams go to null and it is detached from the terminal.
pub fn open(path: &Path) -> Result<()> {
    let (program, args) = LAUNCHER;

    let mut command = Command::new(program);
    command
        .args(args)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    detach(&mut command);

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to run {program}"))?;

    // `setsid` detaches the child from the terminal, not from this process: it stays this
    // process' child, and a zombie for the rest of the session unless someone waits for it.
    std::thread::spawn(move || child.wait());

    Ok(())
}

/// Puts the child in a session of its own.
///
/// Null streams are not enough to keep it away from the terminal: a handler that opens
/// `/dev/tty` itself would otherwise fight bettertree for the keys. Without a controlling
/// terminal to find, it gives up and exits instead.
#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::io;
    use std::os::unix::process::CommandExt;

    // Safe: `setsid` is async-signal-safe, so it is allowed between fork and exec.
    unsafe {
        command.pre_exec(|| match libc::setsid() {
            -1 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        })
    };
}

#[cfg(not(unix))]
fn detach(_command: &mut Command) {}

/// The command line of the default handler for `path`, if that handler wants a terminal.
///
/// Such a handler cannot be spawned like a desktop application: it needs this terminal handed
/// over the way the editor gets it. Anything unexpected along the way means `None`, which leaves
/// the path to the launcher above.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn terminal_command(path: &Path) -> Option<Vec<OsString>> {
    let entry = std::fs::read_to_string(desktop_entry(path)?).ok()?;
    let entry = DesktopEntry::parse(&entry);

    if !entry.terminal {
        return None;
    }

    Some(argv(&entry.exec?, path))
}

/// The desktop knows nothing about terminal applications here.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn terminal_command(_path: &Path) -> Option<Vec<OsString>> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn desktop_entry(path: &Path) -> Option<PathBuf> {
    let mime = xdg_mime(&["query", "filetype", &path.to_string_lossy()])?;
    let name = xdg_mime(&["query", "default", &mime])?;

    data_dirs()
        .map(|dir| dir.join("applications").join(&name))
        .find(|entry| entry.is_file())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn xdg_mime(args: &[&str]) -> Option<String> {
    let output = Command::new("xdg-mime").args(args).output().ok()?;
    let answer = String::from_utf8(output.stdout).ok()?.trim().to_owned();

    (!answer.is_empty()).then_some(answer)
}

/// Where desktop entries live, most specific first, as the XDG base directory spec has them.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn data_dirs() -> impl Iterator<Item = PathBuf> {
    use etcetera::{BaseStrategy, choose_base_strategy};

    let home = choose_base_strategy()
        .ok()
        .map(|strategy| strategy.data_dir());

    let shared = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());

    home.into_iter()
        .chain(shared.split(':').map(PathBuf::from).collect::<Vec<_>>())
}

/// The `Terminal` and `Exec` keys of a desktop entry.
///
/// Only the `[Desktop Entry]` group counts: an action group further down may carry keys of the
/// same name, and those describe something the user did not ask for.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[derive(Default)]
struct DesktopEntry {
    exec: Option<String>,
    terminal: bool,
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl DesktopEntry {
    fn parse(contents: &str) -> Self {
        let mut entry = Self::default();

        for line in contents
            .lines()
            .skip_while(|line| line.trim() != "[Desktop Entry]")
            .skip(1)
            .take_while(|line| !line.trim_start().starts_with('['))
        {
            match line.split_once('=') {
                Some(("Exec", value)) if entry.exec.is_none() => {
                    entry.exec = Some(value.trim().to_owned());
                }
                Some(("Terminal", value)) => entry.terminal = value.trim() == "true",
                _ => {}
            }
        }

        entry
    }
}

/// Expands the `Exec` line into a command line for `path`.
///
/// The field codes that stand for files become the path, the rest describe things a launcher
/// would pass and are dropped. An entry with no file code still gets the path, since the user
/// picked it to open one.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn argv(exec: &str, path: &Path) -> Vec<OsString> {
    let mut argv = Vec::new();
    let mut takes_path = false;

    for word in split_exec(exec) {
        match word.as_str() {
            "%f" | "%F" | "%u" | "%U" => {
                argv.push(path.into());
                takes_path = true;
            }
            _ if word.starts_with('%') => {}
            _ => argv.push(word.into()),
        }
    }

    if !takes_path {
        argv.push(path.into());
    }

    argv
}

/// Splits an `Exec` line into words the way the desktop entry spec has it: whitespace separates
/// them, unless it stands inside double quotes, where a backslash escapes the next character.
///
/// The spec forbids field codes inside a quoted argument, so a quoted word is passed on as it is.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn split_exec(exec: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut escaped = false;

    for char in exec.chars() {
        match char {
            _ if escaped => {
                word.push(char);
                escaped = false;
            }
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            _ if char.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            _ => word.push(char),
        }
    }

    if !word.is_empty() {
        words.push(word);
    }

    words
}

#[cfg(test)]
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_entry_is_recognised_with_its_exec_line() {
        let entry = DesktopEntry::parse("[Desktop Entry]\nExec=hx %F\nTerminal=true\n");

        assert!(entry.terminal);
        assert_eq!(entry.exec.as_deref(), Some("hx %F"));
    }

    #[test]
    fn keys_of_later_groups_are_ignored() {
        let entry = DesktopEntry::parse(
            "[Desktop Entry]\nExec=librewolf %u\nTerminal=false\n\n\
             [Desktop Action new-window]\nExec=hx\nTerminal=true\n",
        );

        assert!(!entry.terminal);
        assert_eq!(entry.exec.as_deref(), Some("librewolf %u"));
    }

    #[test]
    fn the_file_field_code_becomes_the_path() {
        let argv = argv("hx --config c.toml %F", Path::new("/tmp/a.rs"));

        assert_eq!(argv, ["hx", "--config", "c.toml", "/tmp/a.rs"]);
    }

    #[test]
    fn a_quoted_argument_stays_one_word() {
        let argv = argv(r#"sh -c "run it""#, Path::new("/tmp/a.rs"));

        assert_eq!(argv, ["sh", "-c", "run it", "/tmp/a.rs"]);
    }

    #[test]
    fn an_exec_line_without_a_file_code_still_gets_the_path() {
        let argv = argv("btop %i", Path::new("/tmp/a.rs"));

        assert_eq!(argv, ["btop", "/tmp/a.rs"]);
    }
}
