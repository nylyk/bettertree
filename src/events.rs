use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use anyhow::Result;
use crossterm::event::{self, Event as TerminalEvent};

use crate::git::GitInfo;
use crate::tree::scan::Entry;

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
}

impl Events {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();

        let input = sender.clone();
        thread::spawn(move || {
            while let Ok(event) = event::read() {
                if input.send(Event::Input(event)).is_err() {
                    break;
                }
            }
        });

        Self { sender, receiver }
    }

    pub fn sender(&self) -> Sender<Event> {
        self.sender.clone()
    }

    pub fn next(&self) -> Result<Event> {
        Ok(self.receiver.recv()?)
    }
}
