use std::borrow::Cow;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::tree::{NodeId, Tree};

pub enum Outcome {
    Continue,
    Edited,
    Accept,
    Cancel,
}

/// The `/` prompt. It only holds the query; the tree does the filtering, so the results are the
/// tree itself with everything else hidden and the folders above them opened. Stepping through
/// them is `move_up` and `move_down`, which the keymap keeps live while the query is typed.
#[derive(Default)]
pub struct Search {
    input: String,
}

impl Search {
    pub fn clear(&mut self) {
        self.input.clear();
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Esc => Outcome::Cancel,
            KeyCode::Enter => Outcome::Accept,

            KeyCode::Char('u') if ctrl => self.edit(String::clear),
            KeyCode::Char('w') if ctrl => self.edit(delete_component),

            // Nothing left but the `/` itself, so erasing it leaves the prompt.
            KeyCode::Backspace if self.input.is_empty() => Outcome::Cancel,
            KeyCode::Backspace => self.edit(|input| {
                input.pop();
            }),
            KeyCode::Char(char) => self.edit(|input| input.push(char)),

            _ => Outcome::Continue,
        }
    }

    /// The entries matching the query, best first. The tree draws them in its own order, so this
    /// ranking is what decides where the cursor lands. `None` for an empty query, which filters
    /// nothing.
    ///
    /// A query is matched against the entry's own name, so a folder is a result on its own and
    /// not by way of everything it holds; write a separator to match the path instead.
    pub fn matches(&self, tree: &Tree, root: &Path) -> Option<Vec<NodeId>> {
        if self.input.is_empty() {
            return None;
        }

        let by_path = self.input.contains('/');
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(&self.input, CaseMatching::Ignore, Normalization::Smart);
        let mut buffer = Vec::new();

        let mut scored: Vec<(u32, Cow<str>, NodeId)> = tree
            .reachable()
            .filter_map(|(id, node)| {
                let label = match by_path {
                    true => Cow::Owned(relative(&node.path, root)),
                    false => Cow::Borrowed(node.name.as_str()),
                };
                let score = pattern.score(Utf32Str::new(&label, &mut buffer), &mut matcher)?;

                Some((score, label, id))
            })
            .collect();

        // Between equal scores the shorter name is the closer one: `main.rs` before `main_x.rs`.
        scored.sort_by(|(a_score, a, _), (b_score, b, _)| {
            b_score
                .cmp(a_score)
                .then_with(|| a.len().cmp(&b.len()))
                .then_with(|| a.cmp(b))
        });

        // A query aimed at one entry otherwise drags in every name whose letters happen to spell
        // it, scattered: `main` would match `command_line.rs` too. Two thirds of the best keeps
        // the near misses and drops those. A path query shares whole components with its junk,
        // which scores too close to separate this way, so there the ranking has to carry it.
        let floor = scored.first().map_or(0, |(best, _, _)| best * 2 / 3);

        let matched = scored
            .into_iter()
            .take_while(|(score, _, _)| *score >= floor)
            .map(|(_, _, id)| id)
            .collect();

        Some(matched)
    }

    fn edit(&mut self, change: impl FnOnce(&mut String)) -> Outcome {
        change(&mut self.input);

        Outcome::Edited
    }
}

/// The path below the project root, which is what a query with a separator in it matches.
fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Queries are paths, so a word ends at a separator as well as at a space.
fn delete_component(input: &mut String) {
    let trimmed = input.trim_end_matches(['/', ' ']);
    let keep = trimmed
        .rfind(['/', ' '])
        .map_or(0, |separator| separator + 1);

    input.truncate(keep);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::tree::scan::Entry;
    use crate::tree::{Filter, Kind, ROOT};

    fn entry(relative: &str, kind: Kind) -> Entry {
        let path = PathBuf::from("/root").join(relative);

        Entry {
            name: relative
                .rsplit('/')
                .next()
                .expect("a last component")
                .to_owned(),
            path,
            kind,
            symlink: false,
        }
    }

    /// root { src { command_line.rs, main.rs }, .hidden { secret.rs }, README.md }
    fn tree() -> Tree {
        let mut tree = Tree::new(PathBuf::from("/root"));

        tree.graft(
            ROOT,
            vec![
                entry("src", Kind::Dir),
                entry(".hidden", Kind::Dir),
                entry("README.md", Kind::File),
            ],
        );

        let src = tree.find(Path::new("/root/src")).expect("grafted");
        tree.graft(
            src,
            vec![
                entry("src/command_line.rs", Kind::File),
                entry("src/main.rs", Kind::File),
            ],
        );

        let hidden = tree.find(Path::new("/root/.hidden")).expect("grafted");
        tree.graft(hidden, vec![entry(".hidden/secret.rs", Kind::File)]);

        tree
    }

    fn search(query: &str) -> Search {
        let mut search = Search::default();
        for char in query.chars() {
            search.handle_key(KeyEvent::from(KeyCode::Char(char)));
        }

        search
    }

    /// Matches come back ranked, so the tests read them in order.
    fn labels(search: &Search, tree: &Tree) -> Vec<String> {
        search
            .matches(tree, Path::new("/root"))
            .expect("a query")
            .into_iter()
            .map(|id| relative(&tree.node(id).path, Path::new("/root")))
            .collect()
    }

    #[test]
    fn an_empty_query_filters_nothing() {
        assert!(search("").matches(&tree(), Path::new("/root")).is_none());
    }

    #[test]
    fn a_query_finds_the_entry_it_names_and_not_what_its_letters_spell() {
        let tree = tree();

        assert_eq!(labels(&search("main"), &tree), ["src/main.rs"]);
        assert_eq!(labels(&search("readme"), &tree), ["README.md"]);
    }

    #[test]
    fn the_best_match_comes_first() {
        let tree = tree();

        assert_eq!(
            labels(&search("rs"), &tree),
            ["src/main.rs", "src/command_line.rs"],
            "both end in the query, and the shorter name is the closer fit"
        );
    }

    #[test]
    fn a_folder_matches_on_its_own_name_and_not_through_its_contents() {
        let tree = tree();

        assert_eq!(labels(&search("src"), &tree), ["src"]);
    }

    #[test]
    fn a_separator_in_the_query_matches_the_path() {
        let tree = tree();

        // Nothing is named `src/main`, so a hit at all means the path was matched.
        assert_eq!(labels(&search("src/main"), &tree)[0], "src/main.rs");
    }

    #[test]
    fn results_follow_the_filters() {
        let mut tree = tree();

        assert!(labels(&search("secret"), &tree).is_empty());

        tree.set_filter(Filter {
            show_hidden: true,
            ..Filter::default()
        });
        assert_eq!(labels(&search("secret"), &tree), [".hidden/secret.rs"]);
    }
}
