use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::Deserialize;

use super::Kind;
use crate::events::Event;

const MAX_WORKERS: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    #[default]
    FoldersFirst,
    Mixed,
    FilesFirst,
    Type,
}

pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
    pub symlink: bool,
}

struct Request {
    path: PathBuf,
    order: SortOrder,
}

/// Reads directories on worker threads so expanding never blocks the event loop.
pub struct Scanner {
    requests: Sender<Request>,
    in_flight: HashSet<PathBuf>,
}

impl Scanner {
    pub fn new(events: Sender<Event>) -> Self {
        let (requests, queue) = mpsc::channel();
        let queue = Arc::new(Mutex::new(queue));

        for _ in 0..worker_count() {
            let queue = Arc::clone(&queue);
            let events = events.clone();
            thread::spawn(move || work(&queue, &events));
        }

        Self {
            requests,
            in_flight: HashSet::new(),
        }
    }

    pub fn request(&mut self, path: PathBuf, order: SortOrder) {
        if !self.in_flight.insert(path.clone()) {
            return;
        }

        let _ = self.requests.send(Request { path, order });
    }

    pub fn finished(&mut self, path: &Path) {
        self.in_flight.remove(path);
    }
}

fn worker_count() -> usize {
    thread::available_parallelism().map_or(2, |cores| cores.get().min(MAX_WORKERS))
}

fn work(queue: &Mutex<Receiver<Request>>, events: &Sender<Event>) {
    loop {
        // The guard is dropped before reading the directory so other workers can take requests.
        let request = queue.lock().expect("scan queue").recv();

        let Ok(request) = request else {
            return;
        };

        let entries = read_dir(&request.path, request.order);
        let done = Event::ScanDone {
            path: request.path,
            entries,
        };

        if events.send(done).is_err() {
            return;
        }
    }
}

pub fn read_dir(path: &Path, order: SortOrder) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();

        let symlink = file_type.is_symlink();
        let is_dir = match symlink {
            true => fs::metadata(&path).is_ok_and(|meta| meta.is_dir()),
            false => file_type.is_dir(),
        };

        entries.push(Entry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path,
            kind: if is_dir { Kind::Dir } else { Kind::File },
            symlink,
        });
    }

    sort(&mut entries, order);

    Ok(entries)
}

pub fn sort(entries: &mut [Entry], order: SortOrder) {
    entries.sort_by(|a, b| compare(order, a, b));
}

fn compare(order: SortOrder, a: &Entry, b: &Entry) -> Ordering {
    group_order(order, a, b)
        .then_with(|| extension_order(order, a, b))
        .then_with(|| natural_cmp(&a.name, &b.name))
        .then_with(|| a.name.cmp(&b.name))
}

fn group_order(order: SortOrder, a: &Entry, b: &Entry) -> Ordering {
    let a_dir = a.kind == Kind::Dir;
    let b_dir = b.kind == Kind::Dir;

    match order {
        SortOrder::Mixed => Ordering::Equal,
        SortOrder::FilesFirst => a_dir.cmp(&b_dir),
        SortOrder::FoldersFirst | SortOrder::Type => b_dir.cmp(&a_dir),
    }
}

fn extension_order(order: SortOrder, a: &Entry, b: &Entry) -> Ordering {
    if order != SortOrder::Type || a.kind == Kind::Dir || b.kind == Kind::Dir {
        return Ordering::Equal;
    }

    natural_cmp(extension(&a.name), extension(&b.name))
}

fn extension(name: &str) -> &str {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
}

/// Case-insensitive comparison that orders digit runs by value, so `file2` precedes `file10`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();

    loop {
        let (Some(x), Some(y)) = (a.peek().copied(), b.peek().copied()) else {
            return a.next().is_some().cmp(&b.next().is_some());
        };

        if x.is_ascii_digit() && y.is_ascii_digit() {
            let ord = compare_numbers(&take_digits(&mut a), &take_digits(&mut b));
            if ord != Ordering::Equal {
                return ord;
            }
            continue;
        }

        a.next();
        b.next();

        let ord = x.to_lowercase().cmp(y.to_lowercase());
        if ord != Ordering::Equal {
            return ord;
        }
    }
}

fn take_digits(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut digits = String::new();

    while chars.peek().is_some_and(char::is_ascii_digit) {
        digits.push(chars.next().expect("peeked"));
    }

    digits
}

/// Compares digit strings by value without parsing, so arbitrarily long runs cannot overflow.
fn compare_numbers(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');

    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Events;

    fn entry(name: &str, kind: Kind) -> Entry {
        Entry {
            name: name.to_owned(),
            path: PathBuf::from(name),
            kind,
            symlink: false,
        }
    }

    fn sorted(order: SortOrder) -> Vec<String> {
        let mut entries = vec![
            entry("main.rs", Kind::File),
            entry("Cargo.toml", Kind::File),
            entry("src", Kind::Dir),
            entry("README", Kind::File),
            entry("assets", Kind::Dir),
            entry("build.rs", Kind::File),
        ];

        sort(&mut entries, order);

        entries.into_iter().map(|entry| entry.name).collect()
    }

    #[test]
    fn natural_cmp_orders_digit_runs_by_value() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("file10", "file2"), Ordering::Greater);
        assert_eq!(natural_cmp("file007", "file7"), Ordering::Equal);
        assert_eq!(natural_cmp("v1.9.0", "v1.10.0"), Ordering::Less);
    }

    #[test]
    fn natural_cmp_ignores_case() {
        assert_eq!(natural_cmp("Apple", "apple"), Ordering::Equal);
        assert_eq!(natural_cmp("apple", "Banana"), Ordering::Less);
        assert_eq!(natural_cmp("Banana", "apple"), Ordering::Greater);
    }

    #[test]
    fn natural_cmp_orders_prefixes_first() {
        assert_eq!(natural_cmp("file", "file2"), Ordering::Less);
        assert_eq!(natural_cmp("file2", "file"), Ordering::Greater);
    }

    #[test]
    fn folders_first_groups_directories_before_files() {
        assert_eq!(
            sorted(SortOrder::FoldersFirst),
            [
                "assets",
                "src",
                "build.rs",
                "Cargo.toml",
                "main.rs",
                "README"
            ]
        );
    }

    #[test]
    fn files_first_groups_files_before_directories() {
        assert_eq!(
            sorted(SortOrder::FilesFirst),
            [
                "build.rs",
                "Cargo.toml",
                "main.rs",
                "README",
                "assets",
                "src"
            ]
        );
    }

    #[test]
    fn mixed_interleaves_by_name() {
        assert_eq!(
            sorted(SortOrder::Mixed),
            [
                "assets",
                "build.rs",
                "Cargo.toml",
                "main.rs",
                "README",
                "src"
            ]
        );
    }

    #[test]
    fn type_groups_files_by_extension() {
        assert_eq!(
            sorted(SortOrder::Type),
            [
                "assets",
                "src",
                "README",
                "build.rs",
                "main.rs",
                "Cargo.toml"
            ]
        );
    }

    #[test]
    fn scanner_reports_entries_on_the_event_channel() {
        let events = Events::new();
        let mut scanner = Scanner::new(events.sender());
        scanner.request(PathBuf::from("src"), SortOrder::default());

        let Event::ScanDone { path, entries } = events.next().expect("event") else {
            panic!("expected a scan result");
        };

        assert_eq!(path, PathBuf::from("src"));
        let names: Vec<String> = entries
            .expect("readable")
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert!(names.contains(&"main.rs".to_owned()), "{names:?}");
    }
}
