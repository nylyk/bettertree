use std::path::{Path, PathBuf};

use anyhow::Result;
use gix::Repository;
use gix::bstr::BStr;
use gix::status::Item;
use gix::status::index_worktree::Item as WorktreeItem;
use gix::status::plumbing::index_as_worktree::{Change as WorktreeChange, EntryStatus};

use super::{Change, GitInfo};
use crate::git::diff;

/// Builds a full repository snapshot. Runs on a worker thread, so it opens its own repository
/// rather than sharing one with the event loop.
pub fn compute(root: &Path) -> Result<GitInfo> {
    let Ok(repo) = gix::discover(root) else {
        return Ok(GitInfo::none());
    };
    let Some(workdir) = repo.workdir().map(Path::to_path_buf) else {
        return Ok(GitInfo::none());
    };

    let mut info = GitInfo {
        branch: branch(&repo),
        is_repo: true,
        ..GitInfo::default()
    };

    let iter = repo
        .status(gix::progress::Discard)?
        .untracked_files(gix::status::UntrackedFiles::Files)
        .into_iter(None)?;

    for item in iter {
        match item? {
            Item::TreeIndex(change) => record_staged(&mut info, &repo, &workdir, &change),
            Item::IndexWorktree(item) => record_worktree(&mut info, &repo, &workdir, &item),
        }
    }

    info.summarise(&workdir);

    Ok(info)
}

fn branch(repo: &Repository) -> Option<String> {
    if let Ok(Some(name)) = repo.head_name() {
        return Some(name.shorten().to_string());
    }

    let id = repo.head_id().ok()?;

    Some(format!("detached {}", id.shorten_or_id()))
}

fn record_staged(
    info: &mut GitInfo,
    repo: &Repository,
    workdir: &Path,
    change: &gix::diff::index::Change,
) {
    use gix::diff::index::ChangeRef;

    let (location, kind, stat) = match change {
        ChangeRef::Addition { location, id, .. } => {
            (location, Change::Added, diff::count(b"", &blob(repo, id)))
        }
        ChangeRef::Deletion { location, id, .. } => {
            (location, Change::Deleted, diff::count(&blob(repo, id), b""))
        }
        ChangeRef::Modification {
            location,
            previous_id,
            id,
            ..
        } => (
            location,
            Change::Modified,
            diff::count(&blob(repo, previous_id), &blob(repo, id)),
        ),
        ChangeRef::Rewrite {
            location,
            source_id,
            id,
            ..
        } => (
            location,
            Change::Renamed,
            diff::count(&blob(repo, source_id), &blob(repo, id)),
        ),
    };

    info.record(absolute(workdir, location), |status| {
        status.staged = Some(kind);
        status.staged_stat = stat;
    });
}

fn record_worktree(info: &mut GitInfo, repo: &Repository, workdir: &Path, item: &WorktreeItem) {
    match item {
        WorktreeItem::Modification {
            entry,
            rela_path,
            status,
            ..
        } => record_modification(info, repo, workdir, rela_path.as_ref(), entry, status),

        WorktreeItem::DirectoryContents { entry, .. } => {
            if entry.status != gix::dir::entry::Status::Untracked {
                return;
            }

            info.record(absolute(workdir, entry.rela_path.as_ref()), |status| {
                status.untracked = true;
            });
        }

        // Only reachable with rename tracking enabled, which we leave off; treat both halves
        // as what they look like on disk.
        WorktreeItem::Rewrite {
            source,
            dirwalk_entry,
            ..
        } => {
            info.record(absolute(workdir, source.rela_path()), |status| {
                status.unstaged = Some(Change::Deleted);
            });
            info.record(
                absolute(workdir, dirwalk_entry.rela_path.as_ref()),
                |status| status.untracked = true,
            );
        }
    }
}

fn record_modification(
    info: &mut GitInfo,
    repo: &Repository,
    workdir: &Path,
    rela_path: &BStr,
    entry: &gix::index::Entry,
    status: &EntryStatus<(), gix::submodule::Status>,
) {
    let path = absolute(workdir, rela_path);

    match status {
        EntryStatus::Conflict { .. } => info.record(path, |status| status.conflicted = true),

        EntryStatus::Change(WorktreeChange::Removed) => {
            let stat = diff::count(&blob(repo, &entry.id), b"");

            info.record(path, |status| {
                status.unstaged = Some(Change::Deleted);
                status.unstaged_stat = stat;
            });
        }

        EntryStatus::Change(WorktreeChange::Type { .. }) => info.record(path, |status| {
            status.unstaged = Some(Change::Type);
        }),

        EntryStatus::Change(WorktreeChange::Modification { .. }) => {
            let stat = diff::count(&blob(repo, &entry.id), &worktree_file(workdir, rela_path));

            info.record(path, |status| {
                status.unstaged = Some(Change::Modified);
                status.unstaged_stat = stat;
            });
        }

        EntryStatus::Change(WorktreeChange::SubmoduleModification(_)) => {
            info.record(path, |status| status.unstaged = Some(Change::Modified));
        }

        // Neither is a change: one is a stat refresh, the other is an `--intent-to-add`
        // placeholder already reported as a staged addition.
        EntryStatus::NeedsUpdate(_) | EntryStatus::IntentToAdd => {}
    }
}

fn blob(repo: &Repository, id: &gix::hash::oid) -> Vec<u8> {
    repo.find_object(id)
        .ok()
        .map(|object| object.detach().data)
        .unwrap_or_default()
}

fn worktree_file(workdir: &Path, rela_path: &BStr) -> Vec<u8> {
    std::fs::read(absolute(workdir, rela_path)).unwrap_or_default()
}

fn absolute(workdir: &Path, rela_path: &BStr) -> PathBuf {
    workdir.join(gix::path::from_bstr(rela_path).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A throwaway repository built with the git CLI, so the expectations are git's own.
    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("bt-test-{}-{unique}", std::process::id()));

            fs::create_dir_all(&dir).expect("create fixture");
            let dir = dir.canonicalize().expect("canonicalize");

            let fixture = Self { dir };
            fixture.git(&["init", "--quiet"]);
            fixture.git(&["config", "user.email", "test@example.com"]);
            fixture.git(&["config", "user.name", "test"]);

            fixture
        }

        fn git(&self, args: &[&str]) {
            let status = Command::new("git")
                .args(args)
                .current_dir(&self.dir)
                .status()
                .expect("run git");

            assert!(status.success(), "git {args:?} failed");
        }

        fn write(&self, name: &str, contents: &[u8]) {
            let path = self.dir.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }

            fs::write(path, contents).expect("write file");
        }

        fn status_of(&self, name: &str) -> super::super::FileStatus {
            let info = compute(&self.dir).expect("compute");

            *info
                .file(&self.dir.join(name))
                .unwrap_or_else(|| panic!("no status for {name}"))
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn committed() -> Fixture {
        let fixture = Fixture::new();
        fixture.write("tracked.txt", b"one\ntwo\nthree\n");
        fixture.git(&["add", "-A"]);
        fixture.git(&["commit", "--quiet", "-m", "init"]);

        fixture
    }

    #[test]
    fn a_modified_file_is_unstaged_with_line_counts() {
        let fixture = committed();
        fixture.write("tracked.txt", b"one\nTWO\nthree\nfour\n");

        let status = fixture.status_of("tracked.txt");

        assert_eq!(status.code(), " M");
        assert_eq!(status.unstaged_stat.added, 2);
        assert_eq!(status.unstaged_stat.removed, 1);
        assert!(status.staged_stat.is_empty());
    }

    #[test]
    fn a_staged_file_is_staged_with_line_counts() {
        let fixture = committed();
        fixture.write("tracked.txt", b"one\ntwo\nthree\nfour\n");
        fixture.git(&["add", "tracked.txt"]);

        let status = fixture.status_of("tracked.txt");

        assert_eq!(status.code(), "M ");
        assert_eq!(status.staged_stat.added, 1);
        assert_eq!(status.staged_stat.removed, 0);
    }

    #[test]
    fn a_file_staged_then_modified_reports_both_columns() {
        let fixture = committed();
        fixture.write("tracked.txt", b"one\ntwo\nthree\nfour\n");
        fixture.git(&["add", "tracked.txt"]);
        fixture.write("tracked.txt", b"one\ntwo\nthree\nfour\nfive\n");

        let status = fixture.status_of("tracked.txt");

        assert_eq!(status.code(), "MM");
        assert_eq!(status.staged_stat.added, 1);
        assert_eq!(status.unstaged_stat.added, 1);
        assert_eq!(status.stat().added, 2);
    }

    #[test]
    fn an_untracked_file_has_no_line_counts() {
        let fixture = committed();
        fixture.write("new.txt", b"a\nb\n");

        let status = fixture.status_of("new.txt");

        assert_eq!(status.code(), "??");
        assert!(status.untracked);
        assert!(status.stat().is_empty());
    }

    #[test]
    fn a_deleted_file_is_reported_as_deleted() {
        let fixture = committed();
        fs::remove_file(fixture.dir.join("tracked.txt")).expect("remove");

        let status = fixture.status_of("tracked.txt");

        assert_eq!(status.code(), " D");
        assert_eq!(status.unstaged_stat.removed, 3);
    }

    #[test]
    fn a_binary_file_reports_no_line_counts() {
        let fixture = Fixture::new();
        fixture.write("blob.bin", b"\0\x01\x02binary\0");
        fixture.git(&["add", "-A"]);
        fixture.git(&["commit", "--quiet", "-m", "init"]);
        fixture.write("blob.bin", b"\0\x09\x08changed\0");

        let status = fixture.status_of("blob.bin");

        assert_eq!(status.code(), " M");
        assert!(status.stat().is_empty());
    }

    #[test]
    fn changes_roll_up_into_parent_directories() {
        let fixture = committed();
        fixture.write("src/deep/file.txt", b"a\nb\n");
        fixture.git(&["add", "-A"]);
        fixture.git(&["commit", "--quiet", "-m", "nested"]);
        fixture.write("src/deep/file.txt", b"a\nb\nc\n");

        let info = compute(&fixture.dir).expect("compute");

        assert_eq!(
            info.directory(&fixture.dir.join("src")).map(|s| s.added),
            Some(1)
        );
        assert_eq!(
            info.directory(&fixture.dir.join("src/deep"))
                .map(|s| s.added),
            Some(1)
        );
        assert_eq!(info.totals.modified_files, 1);
    }

    #[test]
    fn a_plain_directory_is_not_a_repository() {
        let dir = std::env::temp_dir().join(format!("bt-plain-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create");

        let info = compute(&dir).expect("compute");
        let _ = fs::remove_dir_all(&dir);

        assert!(!info.is_repo());
    }
}
