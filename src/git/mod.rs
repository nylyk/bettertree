pub mod diff;
pub mod ignores;
pub mod status;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;

use crate::events::Event;

pub use ignores::Ignores;

/// The repository's `.git` directory, so it can be watched for staging and commits even when the
/// opened directory sits below the worktree root.
pub fn git_dir(root: &Path) -> Option<PathBuf> {
    gix::discover(root)
        .ok()
        .map(|repo| repo.git_dir().to_path_buf())
}

/// Computes a snapshot off the event loop; the result arrives as `Event::GitDone`.
pub fn spawn(root: PathBuf, events: Sender<Event>) {
    thread::spawn(move || {
        let info = status::compute(&root);
        let _ = events.send(Event::GitDone(Box::new(info)));
    });
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    Added,
    Modified,
    Deleted,
    Renamed,
    Type,
}

impl Change {
    fn letter(self) -> char {
        match self {
            Change::Added => 'A',
            Change::Modified => 'M',
            Change::Deleted => 'D',
            Change::Renamed => 'R',
            Change::Type => 'T',
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct DiffStat {
    pub added: u32,
    pub removed: u32,
}

impl DiffStat {
    pub fn is_empty(self) -> bool {
        self.added == 0 && self.removed == 0
    }

    fn merge(self, other: Self) -> Self {
        Self {
            added: self.added + other.added,
            removed: self.removed + other.removed,
        }
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct FileStatus {
    pub staged: Option<Change>,
    pub unstaged: Option<Change>,
    pub untracked: bool,
    pub conflicted: bool,
    pub staged_stat: DiffStat,
    pub unstaged_stat: DiffStat,
}

impl FileStatus {
    /// The two-column code `git status --short` would print for this path.
    pub fn code(&self) -> String {
        if self.conflicted {
            return "UU".to_owned();
        }
        if self.untracked {
            return "??".to_owned();
        }

        let staged = self.staged.map_or(' ', Change::letter);
        let unstaged = self.unstaged.map_or(' ', Change::letter);

        format!("{staged}{unstaged}")
    }

    pub fn stat(&self) -> DiffStat {
        self.staged_stat.merge(self.unstaged_stat)
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Totals {
    pub staged: DiffStat,
    pub unstaged: DiffStat,
    pub modified_files: usize,
    pub staged_files: usize,
    pub untracked_files: usize,
}

/// A snapshot of the repository, computed off the event loop and then swapped in wholesale.
#[derive(Default)]
pub struct GitInfo {
    pub branch: Option<String>,
    pub totals: Totals,
    files: HashMap<PathBuf, FileStatus>,
    dirs: HashMap<PathBuf, DiffStat>,
    is_repo: bool,
}

impl GitInfo {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn is_repo(&self) -> bool {
        self.is_repo
    }

    pub fn file(&self, path: &Path) -> Option<&FileStatus> {
        self.files.get(path)
    }

    /// `Some` for any directory that contains a change, carrying the totals of everything below it.
    pub fn directory(&self, path: &Path) -> Option<DiffStat> {
        self.dirs.get(path).copied()
    }

    /// True for a changed file and for any directory containing one.
    pub fn has_changes(&self, path: &Path) -> bool {
        self.files.contains_key(path) || self.dirs.contains_key(path)
    }

    fn record(&mut self, path: PathBuf, update: impl FnOnce(&mut FileStatus)) {
        update(self.files.entry(path).or_default());
    }

    /// Rolls per-file changes up into their ancestors and sums the status-bar totals.
    fn summarise(&mut self, workdir: &Path) {
        let mut totals = Totals::default();
        let mut dirs: HashMap<PathBuf, DiffStat> = HashMap::new();

        for (path, status) in &self.files {
            totals.staged = totals.staged.merge(status.staged_stat);
            totals.unstaged = totals.unstaged.merge(status.unstaged_stat);

            if status.untracked {
                totals.untracked_files += 1;
            } else {
                if status.staged.is_some() {
                    totals.staged_files += 1;
                }
                if status.unstaged.is_some() || status.conflicted {
                    totals.modified_files += 1;
                }
            }

            let stat = status.stat();
            for ancestor in path.ancestors().skip(1) {
                let entry = dirs.entry(ancestor.to_path_buf()).or_default();
                *entry = entry.merge(stat);

                if ancestor == workdir {
                    break;
                }
            }
        }

        self.totals = totals;
        self.dirs = dirs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_git_status_short() {
        let staged = FileStatus {
            staged: Some(Change::Modified),
            ..FileStatus::default()
        };
        assert_eq!(staged.code(), "M ");

        let unstaged = FileStatus {
            unstaged: Some(Change::Modified),
            ..FileStatus::default()
        };
        assert_eq!(unstaged.code(), " M");

        let both = FileStatus {
            staged: Some(Change::Added),
            unstaged: Some(Change::Modified),
            ..FileStatus::default()
        };
        assert_eq!(both.code(), "AM");

        let untracked = FileStatus {
            untracked: true,
            ..FileStatus::default()
        };
        assert_eq!(untracked.code(), "??");

        let conflicted = FileStatus {
            conflicted: true,
            unstaged: Some(Change::Modified),
            ..FileStatus::default()
        };
        assert_eq!(conflicted.code(), "UU");
    }

    #[test]
    fn ancestors_aggregate_up_to_the_worktree_root() {
        let workdir = PathBuf::from("/repo");
        let mut info = GitInfo {
            is_repo: true,
            ..GitInfo::default()
        };

        info.record(PathBuf::from("/repo/src/ui/mod.rs"), |status| {
            status.unstaged = Some(Change::Modified);
            status.unstaged_stat = DiffStat {
                added: 3,
                removed: 1,
            };
        });
        info.record(PathBuf::from("/repo/src/main.rs"), |status| {
            status.staged = Some(Change::Modified);
            status.staged_stat = DiffStat {
                added: 5,
                removed: 0,
            };
        });
        info.summarise(&workdir);

        assert_eq!(
            info.directory(Path::new("/repo/src")),
            Some(DiffStat {
                added: 8,
                removed: 1
            })
        );
        assert_eq!(
            info.directory(Path::new("/repo/src/ui")),
            Some(DiffStat {
                added: 3,
                removed: 1
            })
        );
        assert_eq!(info.directory(Path::new("/")), None);

        assert_eq!(info.totals.modified_files, 1);
        assert_eq!(info.totals.staged_files, 1);
        assert_eq!(info.totals.staged.added, 5);
        assert_eq!(info.totals.unstaged.added, 3);
    }
}
