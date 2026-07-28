pub mod scan;
pub mod watch;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use scan::Entry;

pub const ROOT: NodeId = NodeId(0);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(usize);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
    Dir,
}

impl Kind {
    pub fn is_dir(self) -> bool {
        self == Kind::Dir
    }
}

/// Which entries the tree shows. Filters affect display only: nothing is unloaded or skipped.
#[derive(Clone, Copy, Default)]
pub struct Filter {
    pub show_hidden: bool,
    pub show_gitignored: bool,
    pub changed_only: bool,
}

pub struct Node {
    pub name: String,
    pub path: PathBuf,
    pub parent: Option<NodeId>,
    pub kind: Kind,
    pub symlink: bool,
    pub expanded: bool,
    pub children: Option<Vec<NodeId>>,
    pub ignored: Option<bool>,
    /// For a file, it has a git status; for a directory, something below it does.
    pub changed: bool,
    /// Cleared when the watcher sees the path disappear. Nodes are tombstoned rather than
    /// removed so that `NodeId`s, and the expansion state they carry, stay stable.
    pub alive: bool,
}

impl Node {
    pub fn is_loaded(&self) -> bool {
        self.children.is_some()
    }
}

pub struct Row {
    pub depth: usize,
    pub kind: RowKind,
}

/// A row is either an entry or the marker standing in for the entries a folder's display cap
/// left out. The marker is not selectable; the cursor steps over it.
pub enum RowKind {
    Entry(NodeId),
    More(usize),
}

impl Row {
    pub fn entry(&self) -> Option<NodeId> {
        match self.kind {
            RowKind::Entry(id) => Some(id),
            RowKind::More(_) => None,
        }
    }
}

/// A running search, as the tree sees it: the entries that matched, the folders above them, and
/// which of the results the query fits best.
struct SearchFilter {
    matched: HashSet<NodeId>,
    path: HashSet<NodeId>,
    best: Option<NodeId>,
}

/// Nodes are added but never removed, so a collapsed folder keeps the expansion state of
/// everything inside it and re-expanding restores the exact previous shape.
pub struct Tree {
    nodes: Vec<Node>,
    index: HashMap<PathBuf, NodeId>,
    rows: Vec<Row>,
    rows_dirty: bool,
    filter: Filter,
    /// The most entries one folder puts on screen. Display only: the rest stay loaded.
    max_children: usize,
    search: Option<SearchFilter>,
    /// Nodes below these marks have already been classified; everything above is new.
    ignore_mark: usize,
    changed_mark: usize,
    load_mark: usize,
    selected: NodeId,
    selected_row: usize,
}

impl Tree {
    pub fn new(root: PathBuf) -> Self {
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());

        let root = Node {
            name,
            path: root,
            parent: None,
            kind: Kind::Dir,
            symlink: false,
            expanded: true,
            children: None,
            ignored: Some(false),
            changed: false,
            alive: true,
        };

        Self {
            index: HashMap::from([(root.path.clone(), ROOT)]),
            nodes: vec![root],
            rows: Vec::new(),
            rows_dirty: true,
            filter: Filter::default(),
            max_children: usize::MAX,
            search: None,
            ignore_mark: 1,
            changed_mark: 0,
            load_mark: 0,
            selected: ROOT,
            selected_row: 0,
        }
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0]
    }

    pub fn find(&self, path: &Path) -> Option<NodeId> {
        self.index.get(path).copied()
    }

    pub fn selected(&self) -> NodeId {
        self.selected
    }

    pub fn selected_row(&self) -> usize {
        self.selected_row
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn graft(&mut self, id: NodeId, entries: Vec<Entry>) {
        let children = entries
            .into_iter()
            .map(|entry| self.push(id, entry))
            .collect();

        self.nodes[id.0].children = Some(children);
        self.rows_dirty = true;
    }

    fn push(&mut self, parent: NodeId, entry: Entry) -> NodeId {
        let id = NodeId(self.nodes.len());

        self.index.insert(entry.path.clone(), id);
        self.nodes.push(Node {
            name: entry.name,
            path: entry.path,
            parent: Some(parent),
            kind: entry.kind,
            symlink: entry.symlink,
            expanded: false,
            children: None,
            ignored: None,
            changed: false,
            alive: true,
        });

        id
    }

    /// Applies a fresh directory listing to an already-loaded folder. Entries that survive keep
    /// their node, and therefore their expansion and loaded children; the rest are tombstoned.
    /// Returns the paths that disappeared.
    pub fn reconcile(&mut self, id: NodeId, entries: Vec<Entry>) -> Vec<PathBuf> {
        let previous = self.nodes[id.0].children.clone().unwrap_or_default();
        let mut children = Vec::with_capacity(entries.len());

        for entry in entries {
            let survivor = previous.iter().copied().find(|child| {
                let node = &self.nodes[child.0];
                node.name == entry.name && node.kind == entry.kind && node.alive
            });

            match survivor {
                Some(child) => children.push(child),
                None => {
                    let child = self.push(id, entry);
                    children.push(child);
                }
            }
        }

        let mut removed = Vec::new();
        for child in previous {
            if !children.contains(&child) {
                self.kill(child, &mut removed);
            }
        }

        self.nodes[id.0].children = Some(children);
        self.rows_dirty = true;

        removed
    }

    fn kill(&mut self, id: NodeId, removed: &mut Vec<PathBuf>) {
        let node = &mut self.nodes[id.0];
        node.alive = false;

        let path = node.path.clone();
        let children = node.children.clone().unwrap_or_default();

        // A path recreated since then already points at its new node, so only stale entries go.
        if self.index.get(&path) == Some(&id) {
            self.index.remove(&path);
        }
        removed.push(path);

        for child in children {
            self.kill(child, removed);
        }
    }

    /// True when nodes have appeared that have no gitignore verdict yet. Checking this before
    /// classifying matters: building the exclude stack reads files.
    pub fn needs_ignore_classification(&self) -> bool {
        self.ignore_mark < self.nodes.len()
    }

    /// Fills in the gitignore state of newly loaded nodes. Children always sit after their parent
    /// in the arena, so a single forward pass can inherit exclusion from an ignored directory.
    pub fn classify_ignored(&mut self, is_ignored: &mut dyn FnMut(&Path, bool) -> bool) {
        for index in self.ignore_mark..self.nodes.len() {
            let inherited = self.nodes[index]
                .parent
                .is_some_and(|parent| self.nodes[parent.0].ignored == Some(true));

            let node = &self.nodes[index];
            let ignored = inherited || is_ignored(&node.path, node.kind.is_dir());

            self.nodes[index].ignored = Some(ignored);
        }

        self.ignore_mark = self.nodes.len();
        self.rows_dirty = true;
    }

    pub fn set_expanded(&mut self, id: NodeId, expanded: bool) {
        let node = &mut self.nodes[id.0];
        if !node.kind.is_dir() || node.expanded == expanded {
            return;
        }

        node.expanded = expanded;
        self.rows_dirty = true;
    }

    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
        self.rows_dirty = true;
    }

    /// Caps how many entries one folder shows, `0` meaning no cap. What the cap leaves out stays
    /// loaded and searchable; it is summarised by a single marker row, which is what keeps a
    /// folder of a million entries from putting a million rows on screen.
    pub fn set_max_children(&mut self, max: usize) {
        self.max_children = match max {
            0 => usize::MAX,
            max => max,
        };

        self.rows_dirty = true;
    }

    /// The children a folder puts on screen, and how many the cap left out.
    fn visible_children(&self, id: NodeId) -> (Vec<NodeId>, usize) {
        let Some(children) = &self.nodes[id.0].children else {
            return (Vec::new(), 0);
        };

        let selected = self.selected_branch(id);
        let mut shown = Vec::new();
        let mut cut = 0;

        for child in children.iter().copied() {
            if !self.is_visible(child) {
                continue;
            }

            match shown.len() < self.max_children || Some(child) == selected {
                true => shown.push(child),
                false => cut += 1,
            }
        }

        (shown, cut)
    }

    /// Which child of `id`, if any, the cursor sits under. The cap is display only, so it never
    /// takes the selected entry off screen: the branch leading to it is shown past the cap, just
    /// above the marker, and everything remains reachable.
    fn selected_branch(&self, id: NodeId) -> Option<NodeId> {
        let mut current = self.selected;

        while let Some(parent) = self.nodes[current.0].parent {
            if parent == id {
                return Some(current);
            }
            current = parent;
        }

        None
    }

    /// What a folder shows. Nothing past the cap can be selected or opened, so this is also
    /// exactly the set worth reading ahead into.
    pub fn shown_children(&self, id: NodeId) -> Vec<NodeId> {
        self.visible_children(id).0
    }

    /// Narrows the tree to a set of results, best first, and the folders leading to them, or
    /// clears the search when given `None`.
    pub fn set_search(&mut self, ranked: Option<Vec<NodeId>>) {
        self.search = ranked.map(|ranked| SearchFilter {
            path: self.ancestors_of(&ranked),
            best: ranked.first().copied(),
            matched: ranked.into_iter().collect(),
        });

        self.rows_dirty = true;
    }

    /// The folders above the results. They are what the search opens, and walking up stops at
    /// the first one already recorded, so the whole set costs one pass over the results.
    fn ancestors_of(&self, matched: &[NodeId]) -> HashSet<NodeId> {
        let mut path = HashSet::new();

        for id in matched {
            let mut current = self.nodes[id.0].parent;

            while let Some(parent) = current {
                if !path.insert(parent) {
                    break;
                }
                current = self.nodes[parent.0].parent;
            }
        }

        path
    }

    pub fn is_searching(&self) -> bool {
        self.search.is_some()
    }

    /// The results on screen. A folder's display cap can leave some of them out, and what it
    /// leaves out cannot be stepped to, so this counts rows rather than the ranking.
    pub fn match_count(&self) -> usize {
        let Some(search) = &self.search else {
            return 0;
        };

        self.rows
            .iter()
            .filter_map(Row::entry)
            .filter(|id| search.matched.contains(id))
            .count()
    }

    /// Steps between search results, passing over the folders that only lead to one. Walking on
    /// from the cursor and wrapping round is the same as visiting every row once, starting at
    /// the neighbour, so the ends need no special case.
    pub fn move_to_match(&mut self, delta: isize) {
        let count = self.rows.len();
        if count == 0 {
            return;
        }

        let target = (1..=count)
            .map(|step| match delta >= 0 {
                true => (self.selected_row + step) % count,
                false => (self.selected_row + count - step) % count,
            })
            .find(|row| self.is_match(*row));

        if let Some(row) = target {
            self.select_row(row);
        }
    }

    /// Puts the cursor on the result the query fits best, which the tree may well draw below a
    /// weaker one: the rows are in the tree's order, not the ranking's.
    pub fn select_best_match(&mut self) {
        if let Some(best) = self.search.as_ref().and_then(|search| search.best) {
            self.select(best);
        }
    }

    fn is_match(&self, row: usize) -> bool {
        let Some(search) = &self.search else {
            return false;
        };

        self.rows
            .get(row)
            .and_then(Row::entry)
            .is_some_and(|id| search.matched.contains(&id))
    }

    /// Every entry the tree would show if all its folders were open: what a search looks through.
    pub fn reachable(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.iter()
            .filter(|(id, _)| *id != ROOT && self.is_reachable(*id))
    }

    /// Filters hide a folder without hiding what is inside it, so a search has to walk up. The
    /// running search is left out on purpose: every query is matched against the whole tree.
    fn is_reachable(&self, id: NodeId) -> bool {
        let mut current = id;

        while current != ROOT {
            let node = &self.nodes[current.0];
            if !node.alive || !self.passes_filters(node) {
                return false;
            }

            let Some(parent) = node.parent else {
                return false;
            };
            current = parent;
        }

        true
    }

    /// The directories that appeared since the last call and still have not been read. Search
    /// walks the whole project this way, a batch per event, rather than re-scanning the arena.
    pub fn unloaded_directories(&mut self) -> Vec<NodeId> {
        let found = (self.load_mark..self.nodes.len())
            .map(NodeId)
            .filter(|id| {
                let node = &self.nodes[id.0];
                node.kind.is_dir() && !node.is_loaded() && self.is_reachable(*id)
            })
            .collect();

        self.load_mark = self.nodes.len();

        found
    }

    /// Starts the walk over, so entries a filter change has just revealed are read too.
    pub fn rewind_unloaded(&mut self) {
        self.load_mark = 0;
    }

    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.alive)
            .map(|(index, node)| (NodeId(index), node))
    }

    /// Records which nodes carry a git change. Directories are marked when anything below them
    /// changed, which is what lets `changed_only` reveal them.
    pub fn mark_all_changed(&mut self, has_changes: &mut dyn FnMut(&Path) -> bool) {
        self.changed_mark = 0;
        self.mark_changed(has_changes);
    }

    /// Marks only the nodes that appeared since the last pass, which keeps loading a large tree
    /// linear rather than quadratic.
    pub fn mark_changed(&mut self, has_changes: &mut dyn FnMut(&Path) -> bool) {
        for index in self.changed_mark..self.nodes.len() {
            let node = &mut self.nodes[index];
            node.changed = has_changes(&node.path);
        }

        self.changed_mark = self.nodes.len();
        self.rows_dirty = true;
    }

    /// Counts tombstones too, which is fine for the coarse limits it guards.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Collapses a folder and everything inside it, discarding the inner expansion on purpose.
    pub fn collapse_subtree(&mut self, id: NodeId) {
        let mut stack = vec![id];

        while let Some(current) = stack.pop() {
            if let Some(children) = &self.nodes[current.0].children {
                stack.extend(children.iter().copied());
            }

            self.set_expanded(current, false);
        }
    }

    pub fn refresh_rows(&mut self) {
        if !self.rows_dirty {
            return;
        }

        self.rows.clear();
        self.push_rows(ROOT, 0);
        self.rows_dirty = false;

        self.reconcile_selection();
    }

    fn push_rows(&mut self, id: NodeId, depth: usize) {
        let (shown, cut) = self.visible_children(id);

        for child in shown {
            let descend = self.is_open(child);
            self.rows.push(Row {
                depth,
                kind: RowKind::Entry(child),
            });

            if descend {
                self.push_rows(child, depth + 1);
            }
        }

        if cut > 0 {
            self.rows.push(Row {
                depth,
                kind: RowKind::More(cut),
            });
        }
    }

    /// A running search narrows the tree further: only the results and the folders above them.
    fn is_visible(&self, id: NodeId) -> bool {
        let node = &self.nodes[id.0];
        if !node.alive || !self.passes_filters(node) {
            return false;
        }

        match &self.search {
            Some(search) => search.matched.contains(&id) || search.path.contains(&id),
            None => true,
        }
    }

    fn passes_filters(&self, node: &Node) -> bool {
        if !self.filter.show_hidden && node.name.starts_with('.') {
            return false;
        }
        if !self.filter.show_gitignored && node.ignored == Some(true) {
            return false;
        }
        if self.filter.changed_only && !node.changed {
            return false;
        }

        true
    }

    /// `changed_only` and a search both open folders without touching their saved expansion, so
    /// turning either off restores the tree exactly as it was.
    pub fn is_open(&self, id: NodeId) -> bool {
        let node = &self.nodes[id.0];
        if !node.kind.is_dir() {
            return false;
        }

        // A search shows the results and the folders leading to them, and nothing else: a
        // folder that only holds non-matching entries would otherwise open on an empty subtree.
        if let Some(search) = &self.search {
            return search.path.contains(&id);
        }

        node.expanded || (self.filter.changed_only && node.changed)
    }

    /// Keeps the cursor on the selected node, falling back to its nearest visible ancestor.
    fn reconcile_selection(&mut self) {
        let mut candidate = Some(self.selected);

        while let Some(id) = candidate {
            if let Some(row) = self.rows.iter().position(|row| row.entry() == Some(id)) {
                self.selected = id;
                self.selected_row = row;
                return;
            }
            candidate = self.nodes[id.0].parent;
        }

        self.selected = self.rows.first().and_then(Row::entry).unwrap_or(ROOT);
        self.selected_row = 0;
    }

    /// A marker row cannot hold the cursor, so landing on one carries on past it the way the
    /// cursor was already going, and only turns back when that runs out of rows.
    pub fn select_row(&mut self, row: usize) {
        let Some(row) = self.nearest_entry(row, row >= self.selected_row) else {
            return;
        };

        self.selected = self.rows[row].entry().expect("an entry row");
        self.selected_row = row;
    }

    fn nearest_entry(&self, from: usize, downwards: bool) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }

        let is_entry = |row: &usize| self.rows[*row].entry().is_some();

        let below = (from..self.rows.len()).find(is_entry);
        let above = (0..=from.min(self.rows.len() - 1)).rev().find(is_entry);

        match downwards {
            true => below.or(above),
            false => above.or(below),
        }
    }

    /// Puts the cursor on a node, falling back to its nearest visible ancestor. Which entries the
    /// display cap leaves out depends on where the cursor is, so a node with no row of its own
    /// gets one from the rebuild rather than being passed over.
    pub fn select(&mut self, id: NodeId) {
        self.selected = id;
        self.rows_dirty |= !self.rows.iter().any(|row| row.entry() == Some(id));

        match self.rows_dirty {
            true => self.refresh_rows(),
            false => self.reconcile_selection(),
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        let target = self.selected_row.saturating_add_signed(delta);
        self.select_row(target.min(self.rows.len().saturating_sub(1)));
    }

    pub fn move_last(&mut self) {
        self.select_row(self.rows.len().saturating_sub(1));
    }

    /// Moves to the next entry at the current depth or shallower, skipping expanded subtrees.
    pub fn move_next_sibling(&mut self) {
        let Some(depth) = self.selected_depth() else {
            return;
        };

        let target = self.rows[self.selected_row + 1..]
            .iter()
            .position(|row| row.depth <= depth)
            .map(|offset| self.selected_row + 1 + offset);

        if let Some(target) = target {
            self.select_row(target);
        }
    }

    /// Moves to the previous entry at the current depth or shallower, skipping expanded subtrees.
    pub fn move_prev_sibling(&mut self) {
        let Some(depth) = self.selected_depth() else {
            return;
        };

        let target = self.rows[..self.selected_row]
            .iter()
            .rposition(|row| row.depth <= depth);

        if let Some(target) = target {
            self.select_row(target);
        }
    }

    pub fn move_parent(&mut self) {
        if let Some(parent) = self.nodes[self.selected.0].parent
            && parent != ROOT
        {
            self.select(parent);
        }
    }

    fn selected_depth(&self) -> Option<usize> {
        self.rows.get(self.selected_row).map(|row| row.depth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: Kind) -> Entry {
        Entry {
            name: name.to_owned(),
            path: PathBuf::from(name),
            kind,
            symlink: false,
        }
    }

    fn find(tree: &Tree, name: &str) -> NodeId {
        tree.iter()
            .find(|(_, node)| node.name == name)
            .map(|(id, _)| id)
            .expect("live node exists")
    }

    /// root { a { b { deep.rs }, a.rs }, z.rs }
    fn sample() -> Tree {
        let mut tree = Tree::new(PathBuf::from("/root"));

        tree.graft(ROOT, vec![entry("a", Kind::Dir), entry("z.rs", Kind::File)]);
        let a = find(&tree, "a");

        tree.graft(a, vec![entry("b", Kind::Dir), entry("a.rs", Kind::File)]);
        let b = find(&tree, "b");

        tree.graft(b, vec![entry("deep.rs", Kind::File)]);

        tree
    }

    fn visible(tree: &mut Tree) -> Vec<String> {
        tree.refresh_rows();
        tree.rows()
            .iter()
            .map(|row| match row.kind {
                RowKind::Entry(id) => tree.node(id).name.clone(),
                RowKind::More(cut) => format!("…{cut}"),
            })
            .collect()
    }

    /// root { big { one.rs, .two.rs, three.rs, four.rs } }
    fn crowded() -> Tree {
        let mut tree = Tree::new(PathBuf::from("/root"));

        tree.graft(ROOT, vec![entry("big", Kind::Dir)]);
        let big = find(&tree, "big");

        tree.graft(
            big,
            vec![
                entry("one.rs", Kind::File),
                entry(".two.rs", Kind::File),
                entry("three.rs", Kind::File),
                entry("four.rs", Kind::File),
            ],
        );
        tree.set_expanded(big, true);

        tree
    }

    #[test]
    fn a_folder_past_the_cap_shows_what_it_left_out() {
        let mut tree = crowded();
        tree.set_max_children(2);

        assert_eq!(visible(&mut tree), ["big", "one.rs", "three.rs", "…1"]);
    }

    #[test]
    fn the_cap_counts_only_the_entries_a_folder_shows() {
        let mut tree = crowded();
        tree.set_max_children(2);
        tree.set_filter(Filter {
            show_hidden: true,
            ..Filter::default()
        });

        assert_eq!(visible(&mut tree), ["big", "one.rs", ".two.rs", "…2"]);
    }

    #[test]
    fn no_cap_is_set_by_default() {
        let mut tree = crowded();

        assert_eq!(visible(&mut tree), ["big", "one.rs", "three.rs", "four.rs"]);
    }

    #[test]
    fn the_cursor_steps_over_the_marker_the_way_it_was_going() {
        let mut tree = Tree::new(PathBuf::from("/root"));
        tree.set_max_children(2);

        tree.graft(
            ROOT,
            vec![entry("big", Kind::Dir), entry("after.rs", Kind::File)],
        );
        let big = find(&tree, "big");

        tree.graft(
            big,
            vec![
                entry("one.rs", Kind::File),
                entry("two.rs", Kind::File),
                entry("three.rs", Kind::File),
            ],
        );
        tree.set_expanded(big, true);

        assert_eq!(
            visible(&mut tree),
            ["big", "one.rs", "two.rs", "…1", "after.rs"]
        );

        tree.select(find(&tree, "two.rs"));
        tree.move_by(1);
        assert_eq!(tree.node(tree.selected()).name, "after.rs");

        tree.move_by(-1);
        assert_eq!(tree.node(tree.selected()).name, "two.rs");
    }

    #[test]
    fn the_marker_cannot_take_the_cursor_at_the_end_of_the_tree() {
        let mut tree = crowded();
        tree.set_max_children(1);
        tree.refresh_rows();

        tree.move_last();

        assert_eq!(tree.node(tree.selected()).name, "one.rs");
    }

    #[test]
    fn only_the_children_on_screen_are_worth_reading_ahead_into() {
        let mut tree = crowded();
        tree.set_max_children(2);
        let big = find(&tree, "big");

        let shown: Vec<&str> = tree
            .shown_children(big)
            .iter()
            .map(|id| tree.node(*id).name.as_str())
            .collect();

        assert_eq!(shown, ["one.rs", "three.rs"]);
    }

    #[test]
    fn the_cap_never_takes_the_cursor_off_screen() {
        let mut tree = crowded();
        tree.set_max_children(1);
        let four = find(&tree, "four.rs");

        tree.select(four);

        assert_eq!(tree.selected(), four, "the cap is display only");
        assert_eq!(visible(&mut tree), ["big", "one.rs", "four.rs", "…1"]);
    }

    #[test]
    fn the_cap_keeps_the_folder_the_cursor_is_inside() {
        let mut tree = Tree::new(PathBuf::from("/root"));
        tree.set_max_children(1);

        tree.graft(
            ROOT,
            vec![
                entry("a.rs", Kind::File),
                entry("b.rs", Kind::File),
                entry("deep", Kind::Dir),
            ],
        );
        let deep = find(&tree, "deep");

        tree.graft(deep, vec![entry("inner.rs", Kind::File)]);
        tree.set_expanded(deep, true);

        tree.select(find(&tree, "inner.rs"));

        assert_eq!(visible(&mut tree), ["a.rs", "deep", "inner.rs", "…1"]);
    }

    #[test]
    fn the_result_count_leaves_out_what_the_cap_hides() {
        let mut tree = crowded();
        tree.set_max_children(1);

        let matches = vec![
            find(&tree, "one.rs"),
            find(&tree, "three.rs"),
            find(&tree, "four.rs"),
        ];
        tree.set_search(Some(matches));
        tree.select(find(&tree, "big"));
        tree.refresh_rows();

        assert_eq!(visible(&mut tree), ["big", "one.rs", "…2"]);
        assert_eq!(tree.match_count(), 1, "only the results with a row count");
    }

    #[test]
    fn collapsing_preserves_descendant_expansion() {
        let mut tree = sample();
        let a = find(&tree, "a");
        let b = find(&tree, "b");

        tree.set_expanded(a, true);
        tree.set_expanded(b, true);
        assert_eq!(visible(&mut tree), ["a", "b", "deep.rs", "a.rs", "z.rs"]);

        tree.set_expanded(a, false);
        assert_eq!(visible(&mut tree), ["a", "z.rs"]);

        tree.set_expanded(a, true);
        assert_eq!(visible(&mut tree), ["a", "b", "deep.rs", "a.rs", "z.rs"]);
    }

    #[test]
    fn next_sibling_skips_expanded_subtrees() {
        let mut tree = sample();
        let a = find(&tree, "a");
        tree.set_expanded(a, true);
        visible(&mut tree);

        tree.select(a);
        tree.move_next_sibling();

        assert_eq!(tree.node(tree.selected()).name, "z.rs");
    }

    #[test]
    fn next_sibling_leaves_a_subtree_at_its_last_entry() {
        let mut tree = sample();
        let a = find(&tree, "a");
        tree.set_expanded(a, true);
        visible(&mut tree);

        tree.select(find(&tree, "a.rs"));
        tree.move_next_sibling();

        assert_eq!(tree.node(tree.selected()).name, "z.rs");
    }

    #[test]
    fn prev_sibling_steps_back_over_an_expanded_subtree() {
        let mut tree = sample();
        let a = find(&tree, "a");
        tree.set_expanded(a, true);
        visible(&mut tree);

        tree.select(find(&tree, "z.rs"));
        tree.move_prev_sibling();

        assert_eq!(tree.node(tree.selected()).name, "a");
    }

    #[test]
    fn selection_falls_back_to_the_nearest_visible_ancestor() {
        let mut tree = sample();
        let a = find(&tree, "a");
        let b = find(&tree, "b");
        tree.set_expanded(a, true);
        tree.set_expanded(b, true);
        visible(&mut tree);

        tree.select(find(&tree, "deep.rs"));
        tree.set_expanded(a, false);
        visible(&mut tree);

        assert_eq!(tree.node(tree.selected()).name, "a");
    }

    #[test]
    fn hidden_entries_are_filtered_until_asked_for() {
        let mut tree = Tree::new(PathBuf::from("/root"));
        tree.graft(
            ROOT,
            vec![entry(".hidden", Kind::File), entry("shown", Kind::File)],
        );

        assert_eq!(visible(&mut tree), ["shown"]);

        tree.set_filter(Filter {
            show_hidden: true,
            ..Filter::default()
        });
        assert_eq!(visible(&mut tree), [".hidden", "shown"]);
    }

    #[test]
    fn gitignored_entries_are_filtered_until_asked_for() {
        let mut tree = sample();
        let ignored = find(&tree, "z.rs");
        tree.classify_ignored(&mut |path, _| path.ends_with("z.rs"));

        assert_eq!(visible(&mut tree), ["a"]);
        assert_eq!(tree.node(ignored).ignored, Some(true));

        tree.set_filter(Filter {
            show_gitignored: true,
            ..Filter::default()
        });
        assert_eq!(visible(&mut tree), ["a", "z.rs"]);
    }

    #[test]
    fn ignored_directories_pass_exclusion_to_their_children() {
        let mut tree = sample();
        tree.classify_ignored(&mut |path, _| path.ends_with("a"));

        assert_eq!(tree.node(find(&tree, "deep.rs")).ignored, Some(true));
        assert_eq!(tree.node(find(&tree, "z.rs")).ignored, Some(false));
    }

    #[test]
    fn changed_only_reveals_changes_without_altering_expansion() {
        let mut tree = sample();
        let a = find(&tree, "a");
        let b = find(&tree, "b");
        tree.set_expanded(b, true);
        tree.mark_changed(&mut |path| {
            let path = path.to_string_lossy().into_owned();
            ["a", "b", "deep.rs"]
                .iter()
                .any(|name| path.ends_with(name))
        });

        tree.set_filter(Filter {
            changed_only: true,
            ..Filter::default()
        });
        assert_eq!(visible(&mut tree), ["a", "b", "deep.rs"]);
        assert!(tree.is_open(a));
        assert!(
            !tree.node(a).expanded,
            "the filter must not persist expansion"
        );

        tree.set_filter(Filter::default());
        assert_eq!(visible(&mut tree), ["a", "z.rs"]);

        tree.set_expanded(a, true);
        assert_eq!(visible(&mut tree), ["a", "b", "deep.rs", "a.rs", "z.rs"]);
    }

    #[test]
    fn a_search_opens_the_way_to_its_results_without_altering_expansion() {
        let mut tree = sample();
        let a = find(&tree, "a");
        let b = find(&tree, "b");
        let deep = find(&tree, "deep.rs");
        assert_eq!(visible(&mut tree), ["a", "z.rs"]);

        tree.set_search(Some(vec![deep]));
        assert_eq!(visible(&mut tree), ["a", "b", "deep.rs"]);
        assert!(tree.is_open(a) && tree.is_open(b));
        assert!(
            !tree.node(a).expanded,
            "the search must not persist expansion"
        );

        tree.set_search(None);
        assert_eq!(visible(&mut tree), ["a", "z.rs"]);
    }

    #[test]
    fn stepping_through_results_skips_the_folders_and_wraps() {
        let mut tree = sample();
        let deep = find(&tree, "deep.rs");
        let z = find(&tree, "z.rs");

        tree.set_search(Some(vec![deep, z]));
        assert_eq!(visible(&mut tree), ["a", "b", "deep.rs", "z.rs"]);
        tree.select_best_match();
        assert_eq!(tree.selected(), deep);

        tree.move_to_match(1);
        assert_eq!(tree.selected(), z, "the folders in between are passed over");

        tree.move_to_match(1);
        assert_eq!(tree.selected(), deep, "the last result wraps to the first");

        tree.move_to_match(-1);
        assert_eq!(tree.selected(), z, "and the first back to the last");
    }

    #[test]
    fn a_folder_that_is_itself_a_result_stays_shut() {
        let mut tree = sample();
        let b = find(&tree, "b");

        tree.set_search(Some(vec![b]));

        assert_eq!(visible(&mut tree), ["a", "b"]);
        assert!(!tree.is_open(b), "nothing inside it matched");
    }

    #[test]
    fn a_search_looks_through_what_the_filters_leave() {
        let mut tree = Tree::new(PathBuf::from("/root"));
        tree.graft(
            ROOT,
            vec![entry(".git", Kind::Dir), entry("a.rs", Kind::File)],
        );
        tree.graft(find(&tree, ".git"), vec![entry("config", Kind::File)]);

        let names = |tree: &Tree| -> Vec<String> {
            tree.reachable()
                .map(|(_, node)| node.name.clone())
                .collect()
        };

        assert_eq!(names(&tree), ["a.rs"], "a hidden folder hides its contents");

        tree.set_filter(Filter {
            show_hidden: true,
            ..Filter::default()
        });
        assert_eq!(names(&tree), [".git", "a.rs", "config"]);
    }

    #[test]
    fn unloaded_directories_are_reported_once_each() {
        let mut tree = sample();
        tree.graft(find(&tree, "a"), vec![entry("fresh", Kind::Dir)]);

        let first: Vec<String> = tree
            .unloaded_directories()
            .into_iter()
            .map(|id| tree.node(id).name.clone())
            .collect();

        assert_eq!(first, ["fresh"], "the loaded folders are already read");
        assert!(tree.unloaded_directories().is_empty());

        tree.rewind_unloaded();
        assert_eq!(tree.unloaded_directories().len(), 1);
    }

    #[test]
    fn reconciling_keeps_surviving_nodes_and_their_expansion() {
        let mut tree = sample();
        let a = find(&tree, "a");
        let b = find(&tree, "b");
        tree.set_expanded(a, true);
        tree.set_expanded(b, true);
        visible(&mut tree);

        let removed = tree.reconcile(
            a,
            vec![
                entry("b", Kind::Dir),
                entry("a.rs", Kind::File),
                entry("new.rs", Kind::File),
            ],
        );

        assert!(removed.is_empty());
        assert_eq!(find(&tree, "b"), b, "the surviving node must be reused");
        assert!(tree.node(b).expanded, "expansion must survive a rescan");
        assert_eq!(
            visible(&mut tree),
            ["a", "b", "deep.rs", "a.rs", "new.rs", "z.rs"]
        );
    }

    #[test]
    fn reconciling_tombstones_entries_that_disappeared() {
        let mut tree = sample();
        let a = find(&tree, "a");
        let b = find(&tree, "b");
        let deep = find(&tree, "deep.rs");
        tree.set_expanded(a, true);

        let removed = tree.reconcile(a, vec![entry("a.rs", Kind::File)]);

        assert_eq!(removed.len(), 2, "the subtree goes with it: {removed:?}");
        assert!(!tree.node(b).alive);
        assert!(!tree.node(deep).alive);
        assert_eq!(tree.find(&PathBuf::from("b")), None);
        assert_eq!(visible(&mut tree), ["a", "a.rs", "z.rs"]);
    }

    #[test]
    fn reconciling_replaces_a_path_whose_kind_changed() {
        let mut tree = sample();
        let a = find(&tree, "a");
        let b = find(&tree, "b");
        tree.set_expanded(a, true);

        tree.reconcile(a, vec![entry("b", Kind::File), entry("a.rs", Kind::File)]);

        let replacement = find(&tree, "b");
        assert_ne!(replacement, b);
        assert!(!tree.node(b).alive);
        assert_eq!(tree.node(replacement).kind, Kind::File);
    }

    #[test]
    fn reconciling_the_same_listing_changes_nothing() {
        let mut tree = sample();
        let a = find(&tree, "a");
        tree.set_expanded(a, true);
        let before = visible(&mut tree).join(",");

        let removed = tree.reconcile(a, vec![entry("b", Kind::Dir), entry("a.rs", Kind::File)]);

        assert!(removed.is_empty());
        assert_eq!(visible(&mut tree).join(","), before);
    }

    #[test]
    fn a_tombstoned_node_is_skipped_when_iterating() {
        let mut tree = sample();
        let a = find(&tree, "a");
        tree.reconcile(a, Vec::new());

        let names: Vec<&str> = tree.iter().map(|(_, node)| node.name.as_str()).collect();

        assert!(!names.contains(&"deep.rs"), "{names:?}");
        assert!(names.contains(&"a"), "{names:?}");
    }
}
