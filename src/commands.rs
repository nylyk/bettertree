use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    MoveDown,
    MoveUp,
    MoveNextSibling,
    MovePrevSibling,
    MoveParent,
    MoveFirst,
    MoveLast,
    JumpDown,
    JumpUp,
    HalfPageDown,
    HalfPageUp,
    CenterCursor,
    Select,
    ExpandAll,
    CollapseAll,
    Open,
    YankPath,
    YankRelativePath,
    ToggleHidden,
    ToggleGitignored,
    ToggleChangedOnly,
    Refresh,
    Help,
    Quit,
}

/// Aliases are abbreviations of the command name, and only exist where typing the full name in
/// the command bar would be tedious: not for commands that live on a key, not for rare ones.
pub struct Entry {
    pub command: Command,
    pub name: &'static str,
    pub alias: Option<&'static str>,
    pub description: &'static str,
}

const fn entry(
    command: Command,
    name: &'static str,
    alias: Option<&'static str>,
    description: &'static str,
) -> Entry {
    Entry {
        command,
        name,
        alias,
        description,
    }
}

pub const REGISTRY: &[Entry] = &[
    entry(
        Command::MoveDown,
        "move_down",
        None,
        "Move to the next entry, descending into expanded folders",
    ),
    entry(
        Command::MoveUp,
        "move_up",
        None,
        "Move to the previous entry, descending into expanded folders",
    ),
    entry(
        Command::MoveNextSibling,
        "move_next_sibling",
        None,
        "Move to the next entry in this folder, skipping expanded subtrees",
    ),
    entry(
        Command::MovePrevSibling,
        "move_prev_sibling",
        None,
        "Move to the previous entry in this folder, skipping expanded subtrees",
    ),
    entry(
        Command::MoveParent,
        "move_parent",
        None,
        "Move to the containing folder",
    ),
    entry(
        Command::MoveFirst,
        "move_first",
        None,
        "Move to the first entry",
    ),
    entry(
        Command::MoveLast,
        "move_last",
        None,
        "Move to the last entry",
    ),
    entry(
        Command::JumpDown,
        "jump_down",
        Some("jd"),
        "Move down by jump_lines entries",
    ),
    entry(
        Command::JumpUp,
        "jump_up",
        Some("ju"),
        "Move up by jump_lines entries",
    ),
    entry(
        Command::HalfPageDown,
        "half_page_down",
        Some("hpd"),
        "Move down by half a screen",
    ),
    entry(
        Command::HalfPageUp,
        "half_page_up",
        Some("hpu"),
        "Move up by half a screen",
    ),
    entry(
        Command::CenterCursor,
        "center_cursor",
        None,
        "Scroll so the cursor sits in the middle",
    ),
    entry(
        Command::Select,
        "select",
        None,
        "Expand or collapse a folder, or open a file in the editor",
    ),
    entry(
        Command::ExpandAll,
        "expand_all",
        Some("ea"),
        "Expand the focused folder and everything inside it",
    ),
    entry(
        Command::CollapseAll,
        "collapse_all",
        Some("ca"),
        "Collapse the focused folder and everything inside it",
    ),
    entry(
        Command::Open,
        "open",
        None,
        "Open the focused file in the editor",
    ),
    entry(
        Command::YankPath,
        "yank_path",
        Some("yp"),
        "Copy the absolute path to the clipboard",
    ),
    entry(
        Command::YankRelativePath,
        "yank_relative_path",
        Some("yr"),
        "Copy the path relative to the project root",
    ),
    entry(
        Command::ToggleHidden,
        "toggle_hidden",
        Some("th"),
        "Show or hide dotfiles",
    ),
    entry(
        Command::ToggleGitignored,
        "toggle_gitignored",
        Some("tg"),
        "Show or hide gitignored entries",
    ),
    entry(
        Command::ToggleChangedOnly,
        "toggle_changed_only",
        Some("tc"),
        "Show only entries with git changes",
    ),
    entry(
        Command::Refresh,
        "refresh",
        Some("r"),
        "Re-read the loaded folders and the git status",
    ),
    entry(Command::Help, "help", None, "Show the list of commands"),
    entry(Command::Quit, "quit", Some("q"), "Leave bettertree"),
];

impl Command {
    pub fn from_name(name: &str) -> Option<Self> {
        REGISTRY
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.command)
    }
}

fn exact(query: &str) -> Option<&'static Entry> {
    REGISTRY
        .iter()
        .find(|entry| entry.name == query || entry.alias == Some(query))
}

/// Ranks commands for the command bar. An exact name or alias is pinned first so short aliases
/// stay predictable no matter how the fuzzy scores land.
pub fn search(query: &str) -> Vec<&'static Entry> {
    if query.is_empty() {
        return REGISTRY.iter().collect();
    }

    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);

    let mut scored: Vec<(u32, &'static Entry)> = REGISTRY
        .iter()
        .filter_map(|entry| score(entry, &pattern, &mut matcher).map(|score| (score, entry)))
        .collect();

    scored.sort_by(|(a_score, a), (b_score, b)| b_score.cmp(a_score).then(a.name.cmp(b.name)));

    let mut ranked: Vec<&'static Entry> = scored.into_iter().map(|(_, entry)| entry).collect();

    if let Some(exact) = exact(query)
        && let Some(position) = ranked.iter().position(|entry| entry.name == exact.name)
    {
        ranked.remove(position);
        ranked.insert(0, exact);
    }

    ranked
}

fn score(entry: &Entry, pattern: &Pattern, matcher: &mut Matcher) -> Option<u32> {
    let mut buffer = Vec::new();
    let name = pattern.score(Utf32Str::new(entry.name, &mut buffer), matcher);

    let mut alias_buffer = Vec::new();
    let alias = entry
        .alias
        .and_then(|alias| pattern.score(Utf32Str::new(alias, &mut alias_buffer), matcher));

    name.max(alias)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_is_registered_exactly_once() {
        for entry in REGISTRY {
            let matches = REGISTRY
                .iter()
                .filter(|other| other.command == entry.command)
                .count();

            assert_eq!(matches, 1, "{} is registered {matches} times", entry.name);
        }
    }

    #[test]
    fn names_and_aliases_are_unique() {
        let mut labels: Vec<&str> = REGISTRY
            .iter()
            .flat_map(|entry| [Some(entry.name), entry.alias])
            .flatten()
            .collect();

        let count = labels.len();
        labels.sort_unstable();
        labels.dedup();

        assert_eq!(labels.len(), count, "duplicate command name or alias");
    }

    #[test]
    fn aliases_abbreviate_their_command_name() {
        for entry in REGISTRY {
            let Some(alias) = entry.alias else {
                continue;
            };

            let initials: String = entry
                .name
                .split('_')
                .filter_map(|word| word.chars().next())
                .collect();

            assert!(
                entry.name.starts_with(alias) || initials.starts_with(alias),
                "{alias} does not abbreviate {}",
                entry.name
            );
        }
    }

    #[test]
    fn an_empty_query_lists_everything() {
        assert_eq!(search("").len(), REGISTRY.len());
    }

    #[test]
    fn an_exact_alias_ranks_first() {
        assert_eq!(search("q")[0].command, Command::Quit);
        assert_eq!(search("hpd")[0].command, Command::HalfPageDown);
    }

    #[test]
    fn an_exact_name_ranks_first() {
        assert_eq!(search("select")[0].command, Command::Select);
        assert_eq!(search("move_up")[0].command, Command::MoveUp);
    }

    #[test]
    fn a_prefix_finds_the_obvious_command() {
        assert_eq!(search("cent")[0].command, Command::CenterCursor);
        assert_eq!(search("hel")[0].command, Command::Help);
    }

    #[test]
    fn nonsense_matches_nothing() {
        assert!(search("zzzqqq").is_empty());
    }
}
