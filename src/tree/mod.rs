pub mod scan;
pub mod watch;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use scan::Entry;

pub const ROOT: NodeId = NodeId(0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    pub id: NodeId,
    pub depth: usize,
}

/// Nodes are added but never removed, so a collapsed folder keeps the expansion state of
/// everything inside it and re-expanding restores the exact previous shape.
pub struct Tree {
    nodes: Vec<Node>,
    index: HashMap<PathBuf, NodeId>,
    rows: Vec<Row>,
    rows_dirty: bool,
    filter: Filter,
    /// Nodes below these marks have already been classified; everything above is new.
    ignore_mark: usize,
    changed_mark: usize,
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
            ignore_mark: 1,
            changed_mark: 0,
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
        let Some(children) = self.nodes[id.0].children.clone() else {
            return;
        };

        for child in children {
            let node = &self.nodes[child.0];
            if !node.alive || !self.is_visible(node) {
                continue;
            }

            let descend = self.is_open(child);
            self.rows.push(Row { id: child, depth });

            if descend {
                self.push_rows(child, depth + 1);
            }
        }
    }

    fn is_visible(&self, node: &Node) -> bool {
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

    /// `changed_only` opens folders that contain changes without touching their saved expansion,
    /// so turning the filter off restores the tree exactly as it was.
    pub fn is_open(&self, id: NodeId) -> bool {
        let node = &self.nodes[id.0];

        node.expanded || (self.filter.changed_only && node.kind.is_dir() && node.changed)
    }

    /// Keeps the cursor on the selected node, falling back to its nearest visible ancestor.
    fn reconcile_selection(&mut self) {
        let mut candidate = Some(self.selected);

        while let Some(id) = candidate {
            if let Some(row) = self.rows.iter().position(|row| row.id == id) {
                self.selected = id;
                self.selected_row = row;
                return;
            }
            candidate = self.nodes[id.0].parent;
        }

        self.selected = self.rows.first().map_or(ROOT, |row| row.id);
        self.selected_row = 0;
    }

    pub fn select_row(&mut self, row: usize) {
        if let Some(row_entry) = self.rows.get(row) {
            self.selected = row_entry.id;
            self.selected_row = row;
        }
    }

    pub fn select(&mut self, id: NodeId) {
        self.selected = id;
        self.reconcile_selection();
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

    fn visible(tree: &mut Tree) -> Vec<&str> {
        tree.refresh_rows();
        tree.rows()
            .iter()
            .map(|row| tree.node(row.id).name.as_str())
            .collect()
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
