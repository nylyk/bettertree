use std::path::Path;

use gix::Repository;
use gix::worktree::stack::state::ignore::Source;

use crate::tree::Tree;

/// Answers "is this path gitignored?" for the loaded tree. The exclude stack borrows the
/// repository, so it is built per batch and the results are cached on the nodes.
pub struct Ignores {
    repo: Option<Repository>,
}

impl Ignores {
    pub fn open(root: &Path) -> Self {
        Self {
            repo: gix::discover(root).ok(),
        }
    }

    pub fn classify(&self, tree: &mut Tree) {
        let Some(repo) = &self.repo else {
            return;
        };
        let Some(workdir) = repo.workdir().map(Path::to_path_buf) else {
            return;
        };
        let Ok(index) = repo.index_or_empty() else {
            return;
        };
        let Ok(mut stack) = repo.excludes(&index, None, Source::WorktreeThenIdMappingIfNotSkipped)
        else {
            return;
        };

        tree.classify_ignored(&mut |path, is_dir| {
            let Ok(relative) = path.strip_prefix(&workdir) else {
                return false;
            };

            let mode = match is_dir {
                true => gix::index::entry::Mode::DIR,
                false => gix::index::entry::Mode::FILE,
            };

            stack
                .at_entry(relative, Some(mode))
                .map(|platform| platform.is_excluded())
                .unwrap_or(false)
        });
    }
}
