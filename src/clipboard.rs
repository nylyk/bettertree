use std::io::{self, stdout};

use crossterm::clipboard::CopyToClipboard;
use crossterm::execute;

/// Copies through the terminal itself (OSC 52), which also works over SSH and inside tmux.
pub fn copy(text: &str) -> io::Result<()> {
    execute!(stdout(), CopyToClipboard::to_clipboard_from(text))
}
