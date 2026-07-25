use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _, recommended_watcher};

use crate::events::Event;

const DEBOUNCE: Duration = Duration::from_millis(200);

/// Watches exactly the directories that are loaded, so the watched set matches what can be shown.
/// Watches are non-recursive; a new subdirectory starts being watched when it is read.
pub struct Watcher {
    watcher: Option<RecommendedWatcher>,
    watched: HashSet<PathBuf>,
}

impl Watcher {
    pub fn new(enabled: bool, events: Sender<Event>) -> Self {
        Self {
            watcher: match enabled {
                true => start(events),
                false => None,
            },
            watched: HashSet::new(),
        }
    }

    pub fn watch(&mut self, path: &Path) {
        let Some(watcher) = &mut self.watcher else {
            return;
        };
        if !self.watched.insert(path.to_path_buf()) {
            return;
        }

        let _ = watcher.watch(path, RecursiveMode::NonRecursive);
    }

    pub fn unwatch(&mut self, path: &Path) {
        let Some(watcher) = &mut self.watcher else {
            return;
        };
        if !self.watched.remove(path) {
            return;
        }

        let _ = watcher.unwatch(path);
    }
}

fn start(events: Sender<Event>) -> Option<RecommendedWatcher> {
    let (raw, incoming) = mpsc::channel();

    let watcher = recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else {
            return;
        };

        // The inotify backend reports opens as well as writes, and bettertree reads the very
        // directories it watches. Without this filter the first rescan would trigger the next.
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }

        let _ = raw.send(event.paths);
    })
    .ok()?;

    thread::spawn(move || debounce(&incoming, &events));

    Some(watcher)
}

/// Collects notifications and forwards one batch once the filesystem has been quiet for a moment,
/// so a burst like `cargo build` costs a single rescan.
fn debounce(incoming: &Receiver<Vec<PathBuf>>, events: &Sender<Event>) {
    while let Ok(first) = incoming.recv() {
        let mut paths: HashSet<PathBuf> = first.into_iter().collect();

        while let Ok(more) = incoming.recv_timeout(DEBOUNCE) {
            paths.extend(more);
        }

        if events
            .send(Event::FsChange(paths.into_iter().collect()))
            .is_err()
        {
            return;
        }
    }
}
