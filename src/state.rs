use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{Toggles, state_dir};

const VERSION: u32 = 1;

/// Beyond this many bytes a mangled path risks the filesystem's name limit, so it is hashed.
const MAX_NAME: usize = 200;

/// What bettertree remembers about a project between runs. Only the directory opened directly
/// gets a file; directories opened separately keep their own.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub version: u32,
    pub root: PathBuf,
    pub selected: Option<PathBuf>,
    pub expanded: Vec<PathBuf>,
    pub toggles: Toggles,
}

impl State {
    pub fn new(
        root: PathBuf,
        selected: Option<PathBuf>,
        expanded: Vec<PathBuf>,
        toggles: Toggles,
    ) -> Self {
        Self {
            version: VERSION,
            root,
            selected,
            expanded,
            toggles,
        }
    }

    /// Reads the saved state, or `None` when there is nothing usable for this root.
    pub fn load(root: &Path) -> Option<Self> {
        let path = state_path(root).ok()?;
        let text = fs::read_to_string(path).ok()?;
        let state: State = toml::from_str(&text).ok()?;

        // A mismatch means the file name collided with another project; start fresh instead.
        if state.version != VERSION || state.root != root {
            return None;
        }

        Some(state)
    }

    pub fn save(&self) -> Result<()> {
        let path = state_path(&self.root)?;
        let parent = path.parent().context("state path has no parent")?;
        fs::create_dir_all(parent)?;

        let text = toml::to_string(self)?;
        let temporary = path.with_extension("tmp");

        // Written aside and renamed so an interrupted save cannot truncate the old state.
        fs::write(&temporary, text)?;
        fs::rename(&temporary, &path)?;

        Ok(())
    }
}

fn state_path(root: &Path) -> Result<PathBuf> {
    Ok(state_dir()?.join(file_name(root)))
}

/// `/home/nylyk/code/bettertree` becomes `%home%nylyk%code%bettertree.toml`, which is reversible
/// by eye. Very long paths fall back to a hash of the path.
fn file_name(root: &Path) -> String {
    let mangled = root.to_string_lossy().replace(['/', '\\'], "%");

    if mangled.len() <= MAX_NAME {
        return format!("{mangled}.toml");
    }

    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);

    let stem = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    format!("{stem}-{:016x}.toml", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_paths_are_mangled_readably() {
        assert_eq!(
            file_name(Path::new("/home/nylyk/code/bettertree")),
            "%home%nylyk%code%bettertree.toml"
        );
    }

    #[test]
    fn long_paths_fall_back_to_a_hash() {
        let long = PathBuf::from("/".to_owned() + &"segment/".repeat(40) + "project");
        let name = file_name(&long);

        assert!(name.starts_with("project-"), "{name}");
        assert!(name.len() < 40, "{name}");
        assert_eq!(name, file_name(&long), "hashing must be stable");
    }

    #[test]
    fn state_round_trips_through_toml() {
        let state = State {
            version: VERSION,
            root: PathBuf::from("/home/nylyk/code/bettertree"),
            selected: Some(PathBuf::from("src/ui/mod.rs")),
            expanded: vec![PathBuf::from("src"), PathBuf::from("src/ui")],
            toggles: Toggles {
                show_hidden: true,
                show_gitignored: false,
                changed_only: true,
            },
        };

        let text = toml::to_string(&state).expect("serialise");
        let parsed: State = toml::from_str(&text).expect("parse");

        assert_eq!(parsed.root, state.root);
        assert_eq!(parsed.selected, state.selected);
        assert_eq!(parsed.expanded, state.expanded);
        assert!(parsed.toggles.show_hidden);
        assert!(parsed.toggles.changed_only);
        assert!(!parsed.toggles.show_gitignored);
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        assert!(State::load(Path::new("/nonexistent/project/xyzzy")).is_none());
    }
}
