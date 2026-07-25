use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event as TerminalEvent};

use crate::git::GitInfo;
use crate::tree::scan::Entry;

/// How long the input thread waits for a key before checking whether it should step aside.
const POLL: Duration = Duration::from_millis(10);

/// How often `suspend` checks whether the input thread has stepped aside.
const HANDOVER: Duration = Duration::from_millis(20);

pub enum Event {
    Input(TerminalEvent),
    ScanDone {
        path: PathBuf,
        entries: io::Result<Vec<Entry>>,
    },
    GitDone(Box<Result<GitInfo>>),
    FsChange(Vec<PathBuf>),
}

/// Every source of work funnels into one channel so the event loop can block instead of polling.
pub struct Events {
    sender: Sender<Event>,
    receiver: Receiver<Event>,
    input: Arc<Input>,
}

/// Lets the input thread be taken off stdin. Without this it keeps reading while a child process
/// such as the editor is in the foreground, and the two of them split the user's keystrokes.
struct Input {
    suspended: AtomicBool,
    on_stdin: AtomicBool,
}

impl Events {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();

        let input = Arc::new(Input {
            suspended: AtomicBool::new(false),
            on_stdin: AtomicBool::new(false),
        });

        let control = Arc::clone(&input);
        let forward = sender.clone();
        thread::spawn(move || read_input(&control, &forward));

        Self {
            sender,
            receiver,
            input,
        }
    }

    pub fn sender(&self) -> Sender<Event> {
        self.sender.clone()
    }

    pub fn next(&self) -> Result<Event> {
        Ok(self.receiver.recv()?)
    }

    /// Hands stdin to another program. Blocks until the input thread has actually let go, so the
    /// child receives every key rather than every other one.
    pub fn suspend(&self) {
        self.input.suspended.store(true, Ordering::SeqCst);

        while self.input.on_stdin.load(Ordering::SeqCst) {
            thread::sleep(HANDOVER);
        }
    }

    pub fn resume(&self) {
        self.input.suspended.store(false, Ordering::SeqCst);
    }
}

fn read_input(control: &Input, events: &Sender<Event>) {
    loop {
        if control.suspended.load(Ordering::SeqCst) {
            control.on_stdin.store(false, Ordering::SeqCst);
            thread::sleep(HANDOVER);
            continue;
        }

        control.on_stdin.store(true, Ordering::SeqCst);

        // Polling rather than a blocking read, so suspending never has to wait for a keypress.
        match event::poll(POLL) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => return,
        }

        let Ok(event) = event::read() else {
            return;
        };

        if events.send(Event::Input(event)).is_err() {
            return;
        }
    }
}
