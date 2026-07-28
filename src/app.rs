use std::collections::HashSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Result;
use crossterm::event::{Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::clipboard;
use crate::command_line::{CommandLine, Outcome};
use crate::commands::{Command, REGISTRY};
use crate::config::{Config, Toggles};
use crate::editor;
use crate::events::{Event, Events};
use crate::foreground;
use crate::git::{self, GitInfo, Ignores};
use crate::opener;
use crate::search::{self, Search};
use crate::state::State;
use crate::tree::scan::{Entry, Scanner};
use crate::tree::watch::Watcher;
use crate::tree::{Filter, NodeId, ROOT, Row, Tree};
use crate::ui;

/// Search reads the folders it has not seen yet, and stops reading once the project is this big.
const SEARCH_LOAD_LIMIT: usize = 50_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Command,
    Search,
    Help,
}

pub struct App {
    pub root: PathBuf,
    pub tree: Tree,
    pub config: Config,
    pub mode: Mode,
    pub command_line: CommandLine,
    pub search: Search,
    /// Where the cursor was when the search opened, so cancelling puts it back.
    search_return: Option<NodeId>,
    pub help_scroll: usize,
    pub scroll: usize,
    pub message: Option<String>,
    pub git: GitInfo,
    pub git_pending: bool,
    pub toggles: Toggles,
    pending_expand: Vec<PathBuf>,
    expansion: Option<Expansion>,
    pending_select: Option<PathBuf>,
    events: Events,
    scanner: Scanner,
    ignores: Ignores,
    watcher: Watcher,
    viewport_height: usize,
    pending_foreground: Option<Vec<OsString>>,
    quit: bool,
}

impl App {
    pub fn new(root: PathBuf, config: Config) -> Self {
        let events = Events::new();
        let scanner = Scanner::new(events.sender());
        let watcher = Watcher::new(config.watch, events.sender());
        let ignores = Ignores::open(&root);

        let mut tree = Tree::new(root.clone());
        tree.set_max_children(config.max_children);

        Self {
            tree,
            toggles: config.toggles,
            root,
            config,
            mode: Mode::Normal,
            command_line: CommandLine::new(),
            search: Search::default(),
            search_return: None,
            help_scroll: 0,
            scroll: 0,
            message: None,
            git: GitInfo::none(),
            git_pending: false,
            pending_expand: Vec::new(),
            expansion: None,
            pending_select: None,
            ignores,
            watcher,
            events,
            scanner,
            viewport_height: 1,
            pending_foreground: None,
            quit: false,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        self.load_root();

        while !self.quit {
            if self.tree.needs_ignore_classification() {
                self.ignores.classify(&mut self.tree);
            }
            if self.mode == Mode::Search {
                self.load_unread_folders();
            }
            self.tree.refresh_rows();
            self.viewport_height = usize::from(terminal.size()?.height)
                .saturating_sub(2)
                .max(1);
            self.clamp_scroll();

            terminal.draw(|frame| ui::render(self, frame))?;

            match self.events.next()? {
                Event::Input(TerminalEvent::Key(key)) => self.handle_key(key),
                Event::Input(_) => {}
                Event::ScanDone { path, entries } => self.scan_done(path, entries),
                Event::GitDone { root, info } => self.git_done(&root, *info),
                Event::FsChange(paths) => self.fs_change(paths),
            }

            if let Some(argv) = self.pending_foreground.take() {
                self.save();

                self.events.suspend();
                let ran = foreground::run(terminal, &argv);
                self.events.resume();

                if let Err(err) = ran {
                    self.message = Some(format!("{err:#}"));
                }
                self.refresh_git();
            }
        }

        self.save();

        Ok(())
    }

    /// Brings up the tree of `self.root`: its saved state, its contents, and its git status.
    fn load_root(&mut self) {
        if let Some(git_dir) = git::git_dir(&self.root) {
            self.watcher.watch(&git_dir);
        }

        self.restore();
        self.request(ROOT);
        self.apply_filter();
        self.refresh_git();
    }

    /// Picks up where the last session in this directory left off.
    fn restore(&mut self) {
        let Some(state) = State::load(&self.root) else {
            return;
        };

        self.toggles = state.toggles;
        self.pending_expand = state
            .expanded
            .iter()
            .map(|relative| self.root.join(relative))
            .collect();
        self.pending_select = state.selected.map(|relative| self.root.join(relative));
    }

    /// Expansion is restored as the directories arrive, since a path only becomes a node once
    /// its parent has been read.
    fn apply_restored(&mut self) {
        let mut waiting = Vec::new();

        for path in std::mem::take(&mut self.pending_expand) {
            match self.tree.find(&path) {
                Some(id) => self.expand(id),
                None => waiting.push(path),
            }
        }
        self.pending_expand = waiting;

        if let Some(path) = self.pending_select.clone()
            && let Some(id) = self.tree.find(&path)
        {
            self.tree.select(id);
            self.pending_select = None;
        }
    }

    fn save(&mut self) {
        let expanded = self.expanded_paths();
        let selected = self
            .relative(&self.tree.node(self.tree.selected()).path)
            .filter(|path| !path.as_os_str().is_empty());

        let state = State::new(self.root.clone(), selected, expanded, self.toggles);

        if let Err(err) = state.save() {
            self.message = Some(format!("could not save state: {err:#}"));
        }
    }

    fn expanded_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self
            .tree
            .iter()
            .filter(|(id, node)| *id != ROOT && node.expanded && node.kind.is_dir())
            .filter_map(|(_, node)| self.relative(&node.path))
            .collect();

        paths.sort();

        paths
    }

    fn relative(&self, path: &Path) -> Option<PathBuf> {
        path.strip_prefix(&self.root).ok().map(Path::to_path_buf)
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }

        match self.mode {
            Mode::Normal => self.handle_normal_key(key),
            Mode::Command => self.handle_command_key(key),
            Mode::Search => self.handle_search_key(key),
            Mode::Help => self.handle_help_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        if let Some(command) = self.config.keys.get(key) {
            self.dispatch(command);
        }
    }

    fn handle_command_key(&mut self, key: KeyEvent) {
        if let Some(command) = self.prompt_command(key) {
            self.dispatch(command);
            return;
        }

        let outcome = self.command_line.handle_key(key);
        self.command_line_outcome(outcome);
    }

    fn command_line_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Continue => {}
            Outcome::Cancel => self.dispatch(Command::Dismiss),
            Outcome::Error(message) => self.message = Some(message),
            Outcome::Run(command) => {
                self.mode = Mode::Normal;
                self.dispatch(command);
            }
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        if let Some(command) = self.prompt_command(key) {
            self.dispatch(command);
            return;
        }

        match self.search.handle_key(key) {
            search::Outcome::Continue => {}
            search::Outcome::Edited => self.requery(),
            search::Outcome::Accept => self.dispatch(Command::Select),
            search::Outcome::Cancel => self.dispatch(Command::Dismiss),
        }
    }

    /// The overlay has no input of its own, so every key it knows reaches the keymap.
    fn handle_help_key(&mut self, key: KeyEvent) {
        if let Some(command) = self.config.keys.get(key).filter(acts_on_top) {
            self.dispatch(command);
        }
    }

    /// What a prompt lets through to the keymap. A key it can take as input is input, so binding
    /// a letter never stops it from being typed.
    fn prompt_command(&self, key: KeyEvent) -> Option<Command> {
        let typed = matches!(key.code, KeyCode::Char(_))
            && key.modifiers.difference(KeyModifiers::SHIFT).is_empty();

        match typed {
            true => None,
            false => self.config.keys.get(key).filter(acts_on_top),
        }
    }

    /// The single place where a command becomes an action; keys and the command bar both land here.
    pub fn dispatch(&mut self, command: Command) {
        self.message = None;

        match command {
            Command::MoveDown => self.move_by(1),
            Command::MoveUp => self.move_by(-1),
            Command::MoveNextSibling => self.tree.move_next_sibling(),
            Command::MovePrevSibling => self.tree.move_prev_sibling(),
            Command::MoveParent => self.tree.move_parent(),
            Command::MoveFirst => self.tree.select_row(0),
            Command::MoveLast => self.tree.move_last(),
            Command::JumpDown => self.move_cursor_by(self.config.jump_lines as isize),
            Command::JumpUp => self.move_cursor_by(-(self.config.jump_lines as isize)),
            Command::HalfPageDown => self.move_cursor_by((self.viewport_height / 2) as isize),
            Command::HalfPageUp => self.move_cursor_by(-((self.viewport_height / 2) as isize)),
            Command::CenterCursor => self.center_cursor(),

            Command::Select => self.select(),
            Command::Open => self.open_selected(),
            Command::Cd => self.cd(),
            Command::CdUp => self.cd_up(),
            Command::ExpandAll => self.expand_all(),
            Command::CollapseAll => self.collapse_all(),
            Command::YankPath => self.yank(YankKind::Absolute),
            Command::YankRelativePath => self.yank(YankKind::Relative),

            Command::ToggleHidden => self.toggle(Toggle::Hidden),
            Command::ToggleGitignored => self.toggle(Toggle::Gitignored),
            Command::ToggleChangedOnly => self.toggle(Toggle::ChangedOnly),
            Command::Refresh => self.refresh(),

            Command::OpenCommandBar => self.open_command_bar(),
            Command::Search => self.open_search(),
            Command::Dismiss => self.dismiss(),
            Command::Help => self.show_help(),
            Command::Quit => self.quit = true,
        }
    }

    /// Moving walks whatever list is on top: the help overlay, the completions, the results of
    /// a running search, or the tree.
    fn move_by(&mut self, delta: isize) {
        match self.mode {
            Mode::Help => self.scroll_help(delta),
            Mode::Command => self.command_line.highlight(delta),
            Mode::Search => self.tree.move_to_match(delta),
            Mode::Normal => self.tree.move_by(delta),
        }
    }

    fn scroll_help(&mut self, delta: isize) {
        self.help_scroll = self
            .help_scroll
            .saturating_add_signed(delta)
            .min(REGISTRY.len().saturating_sub(1));
    }

    fn move_cursor_by(&mut self, delta: isize) {
        let row = self.tree.selected_row().saturating_add_signed(delta);
        self.tree
            .select_row(row.min(self.tree.rows().len().saturating_sub(1)));
    }

    fn center_cursor(&mut self) {
        self.scroll = self
            .tree
            .selected_row()
            .saturating_sub(self.viewport_height / 2);
    }

    /// Keeps the cursor inside the viewport with `scrolloff` lines of context where possible.
    fn clamp_scroll(&mut self) {
        let rows = self.tree.rows().len();
        let cursor = self.tree.selected_row();
        let scrolloff = self
            .config
            .scrolloff
            .min(self.viewport_height.saturating_sub(1) / 2);

        if cursor < self.scroll + scrolloff {
            self.scroll = cursor.saturating_sub(scrolloff);
        }
        if cursor + scrolloff >= self.scroll + self.viewport_height {
            self.scroll = cursor + scrolloff + 1 - self.viewport_height;
        }

        self.scroll = self.scroll.min(rows.saturating_sub(self.viewport_height));
    }

    /// Taking whatever the cursor is on: a candidate, a result, or an entry of the tree.
    fn select(&mut self) {
        match self.mode {
            Mode::Command => {
                let outcome = self.command_line.run();
                self.command_line_outcome(outcome);
            }
            Mode::Search => self.select_match(),
            Mode::Help => {}
            Mode::Normal => self.select_entry(),
        }
    }

    fn select_entry(&mut self) {
        let id = self.tree.selected();
        let node = self.tree.node(id);

        if !node.kind.is_dir() {
            self.edit_selected();
            return;
        }

        match node.expanded {
            true => self.collapse(id),
            false => self.expand(id),
        }
    }

    fn edit_selected(&mut self) {
        let Some(path) = self.selected_file() else {
            return;
        };

        match editor::command(&self.config.editor, &path) {
            Ok(argv) => self.pending_foreground = Some(argv),
            Err(err) => self.message = Some(format!("{err:#}")),
        }
    }

    /// A handler that wants a terminal gets this one, the way the editor does. Everything else is
    /// a desktop application and is launched away from the terminal.
    fn open_selected(&mut self) {
        let Some(path) = self.selected_file() else {
            return;
        };

        if let Some(argv) = opener::terminal_command(&path) {
            self.pending_foreground = Some(argv);
            return;
        }

        if let Err(err) = opener::open(&path) {
            self.message = Some(format!("{err:#}"));
        }
    }

    fn cd(&mut self) {
        let Some(id) = self.focused_folder() else {
            return;
        };
        let path = self.tree.node(id).path.clone();

        self.reroot(path);
    }

    fn cd_up(&mut self) {
        let Some(parent) = self.root.parent().map(Path::to_path_buf) else {
            self.message = Some("no folder above this one".to_owned());
            return;
        };

        self.reroot(parent);
    }

    /// Starts over on another root: a fresh arena, its own gitignore and git status, and the state
    /// saved for it. The root being left is saved first, so returning to it finds it as it was.
    ///
    /// Scans of the old tree may still be in flight. They land on paths this arena does not know
    /// yet, or does not know at all, and `scan_done` drops those.
    fn reroot(&mut self, root: PathBuf) {
        self.save();
        self.watcher.unwatch_all();

        self.root = root;
        self.tree = Tree::new(self.root.clone());
        self.tree.set_max_children(self.config.max_children);
        self.ignores = Ignores::open(&self.root);

        self.mode = Mode::Normal;
        self.search.clear();
        self.search_return = None;
        self.expansion = None;
        self.scroll = 0;

        self.load_root();
    }

    fn selected_file(&mut self) -> Option<PathBuf> {
        let node = self.tree.node(self.tree.selected());

        if node.kind.is_dir() {
            self.message = Some("not a file".to_owned());
            return None;
        }

        Some(node.path.clone())
    }

    fn expand_all(&mut self) {
        let Some(id) = self.focused_folder() else {
            return;
        };

        // One at a time: a walk still waiting on directories is abandoned, budget and all.
        self.expansion = None;
        self.expand_subtree(id, Expansion::default());
    }

    /// Expands everything currently known below `id`. Directories that have not been read yet
    /// stop the walk; it resumes from them in `scan_done`, so the subtree opens as it loads.
    ///
    /// The expansion is kept only while it still has a directory to wait for. Once it has none,
    /// it is over: a later rescan of the subtree cannot restart the walk, and the budget it spent
    /// cannot count against the next `expand_all`.
    fn expand_subtree(&mut self, id: NodeId, mut expansion: Expansion) {
        let limit = self.expand_limit();
        let mut stack = vec![id];

        while let Some(current) = stack.pop() {
            if expansion.entries > limit {
                self.message = Some(format!("stopped expanding at {limit} entries"));
                return;
            }

            let node = self.tree.node(current);
            let unread = !node.is_loaded();
            let path = node.path.clone();

            self.expand(current);

            // The walk cannot see past a directory it has not read, so it stops and waits.
            if unread {
                expansion.waiting.insert(path);
                continue;
            }

            // Only what the folder shows: entries the display cap or a filter left out are not
            // opened, and do not spend the budget either.
            let shown = self.tree.shown_children(current);
            expansion.entries += shown.len();

            let subdirectories = shown
                .into_iter()
                .filter(|child| self.tree.node(*child).kind.is_dir());

            stack.extend(subdirectories);
        }

        if !expansion.waiting.is_empty() {
            self.expansion = Some(expansion);
        }
    }

    /// How many entries one `expand_all` may open. The per-folder cap bounds how wide a folder
    /// gets, not how many folders a subtree holds, so this is what keeps `expand_all` on a tree
    /// full of directories from filling the row list. `0` in the config turns it off.
    fn expand_limit(&self) -> usize {
        match self.config.max_expand_all {
            0 => usize::MAX,
            limit => limit,
        }
    }

    /// Takes the running `expand_all` when it was waiting on this directory, so a scan of any
    /// other path — a manual `:refresh`, or the watcher — cannot restart a finished walk.
    fn resumed_expansion(&mut self, path: &Path) -> Option<Expansion> {
        let mut expansion = self.expansion.take()?;

        if expansion.waiting.remove(path) {
            return Some(expansion);
        }

        self.expansion = Some(expansion);

        None
    }

    fn collapse_all(&mut self) {
        let Some(id) = self.focused_folder() else {
            return;
        };

        self.expansion = None;
        self.tree.collapse_subtree(id);
        self.tree.select(id);
    }

    /// The folder the focused row belongs to: itself if it is one, otherwise its parent.
    fn focused_folder(&self) -> Option<NodeId> {
        let id = self.tree.selected();
        let node = self.tree.node(id);

        match node.kind.is_dir() {
            true => Some(id),
            false => node.parent,
        }
    }

    fn yank(&mut self, kind: YankKind) {
        let path = self.tree.node(self.tree.selected()).path.clone();
        let text = match kind {
            YankKind::Absolute => path.to_string_lossy().into_owned(),
            YankKind::Relative => self
                .relative(&path)
                .unwrap_or_else(|| path.clone())
                .to_string_lossy()
                .into_owned(),
        };

        match clipboard::copy(&text) {
            Ok(()) => self.message = Some(format!("yanked {text}")),
            Err(err) => self.message = Some(format!("could not yank: {err}")),
        }
    }

    fn toggle(&mut self, toggle: Toggle) {
        if toggle.needs_git() && !self.git.is_repo() {
            self.message = Some("not a git repository".to_owned());
            return;
        }

        match toggle {
            Toggle::Hidden => self.toggles.show_hidden = !self.toggles.show_hidden,
            Toggle::Gitignored => self.toggles.show_gitignored = !self.toggles.show_gitignored,
            Toggle::ChangedOnly => self.toggles.changed_only = !self.toggles.changed_only,
        }

        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        self.tree.set_filter(Filter {
            show_hidden: self.toggles.show_hidden,
            show_gitignored: self.toggles.show_gitignored,
            changed_only: self.toggles.changed_only,
        });

        // The results were matched against the old filters, so they are worked out again.
        if self.tree.is_searching() {
            self.refresh_matches();
        }

        if self.toggles.changed_only {
            self.load_changed_directories();
        }

        self.prefetch_shown();
    }

    /// A filter change puts a different set of entries on screen, so the look-ahead catches up:
    /// folders it passed over while they were hidden can be opened now.
    fn prefetch_shown(&mut self) {
        self.tree.refresh_rows();

        let open: Vec<NodeId> = self
            .tree
            .rows()
            .iter()
            .filter_map(Row::entry)
            .filter(|id| self.tree.is_open(*id))
            .collect();

        self.prefetch(ROOT);

        for id in open {
            self.prefetch(id);
        }
    }

    /// `changed_only` can reveal folders that were never expanded, so their contents are loaded
    /// on demand rather than being left as empty rows.
    fn load_changed_directories(&mut self) {
        let pending: Vec<PathBuf> = self
            .tree
            .iter()
            .filter(|(_, node)| node.changed && node.kind.is_dir() && !node.is_loaded())
            .map(|(_, node)| node.path.clone())
            .collect();

        for path in pending {
            self.scanner.request(path, self.config.sort_order);
        }
    }

    /// Re-reads every loaded directory and the git status. The watcher normally keeps both up to
    /// date; this is the manual path for `watch = false` and for anything the watcher missed.
    fn refresh(&mut self) {
        let loaded: Vec<PathBuf> = self
            .tree
            .iter()
            .filter(|(_, node)| node.kind.is_dir() && node.is_loaded())
            .map(|(_, node)| node.path.clone())
            .collect();

        for path in loaded {
            self.scanner.request(path, self.config.sort_order);
        }

        self.refresh_git();
    }

    fn open_command_bar(&mut self) {
        self.command_line.open();
        self.mode = Mode::Command;
    }

    fn open_search(&mut self) {
        self.search.clear();
        self.search_return = Some(self.tree.selected());
        self.mode = Mode::Search;

        self.tree.rewind_unloaded();
        self.requery();
    }

    /// A new query is a different set of results, so the cursor goes to the best of them.
    fn requery(&mut self) {
        self.refresh_matches();

        self.tree.refresh_rows();
        self.tree.select_best_match();
    }

    /// Re-filters without moving the cursor, for entries that turn up while the query stands.
    fn refresh_matches(&mut self) {
        let matches = self.search.matches(&self.tree, &self.root);
        self.tree.set_search(matches);
    }

    /// A search can only find what the arena holds, so it reads the folders that were never
    /// opened. The scanner threads do the reading and results grow as they arrive.
    fn load_unread_folders(&mut self) {
        if self.tree.node_count() > SEARCH_LOAD_LIMIT {
            self.message = Some(format!("searching the first {SEARCH_LOAD_LIMIT} entries"));
            return;
        }

        for id in self.tree.unloaded_directories() {
            self.request(id);
        }
    }

    /// Taking a result ends the search on the spot: the tree comes back whole with the cursor
    /// on that entry. A query that found nothing has nothing to take, so it just goes back.
    fn select_match(&mut self) {
        if self.tree.match_count() == 0 {
            self.cancel_search();
            return;
        }

        let selected = self.tree.selected();

        self.leave_search();
        self.reveal(selected);
    }

    fn cancel_search(&mut self) {
        let restore = self.search_return;

        self.leave_search();

        if let Some(id) = restore {
            self.tree.select(id);
        }
    }

    fn leave_search(&mut self) {
        self.mode = Mode::Normal;
        self.search_return = None;
        self.search.clear();
        self.tree.set_search(None);
    }

    /// Opens every folder above a node and puts the cursor on it, in the middle of the screen.
    fn reveal(&mut self, id: NodeId) {
        let mut current = self.tree.node(id).parent;

        while let Some(parent) = current {
            self.expand(parent);
            current = self.tree.node(parent).parent;
        }

        self.tree.select(id);
        self.tree.refresh_rows();
        self.center_cursor();
    }

    /// Backs out of whatever is on top of the tree.
    fn dismiss(&mut self) {
        match self.mode {
            Mode::Search => self.cancel_search(),
            Mode::Command | Mode::Help => self.mode = Mode::Normal,
            Mode::Normal => {}
        }
    }

    fn show_help(&mut self) {
        self.help_scroll = 0;
        self.mode = Mode::Help;
    }

    fn expand(&mut self, id: NodeId) {
        self.tree.set_expanded(id, true);

        match self.tree.node(id).is_loaded() {
            true => self.prefetch(id),
            false => self.request(id),
        }
    }

    fn collapse(&mut self, id: NodeId) {
        self.expansion = None;
        self.tree.set_expanded(id, false);
    }

    fn request(&mut self, id: NodeId) {
        let path = self.tree.node(id).path.clone();
        self.scanner.request(path, self.config.sort_order);
    }

    /// Loads the direct children of every subdirectory so the next expand is instant. Only
    /// expanded directories prefetch, which is what keeps this from walking the whole tree, and
    /// only the subdirectories on screen: what the display cap left out cannot be opened.
    fn prefetch(&mut self, id: NodeId) {
        for child in self.tree.shown_children(id) {
            let node = self.tree.node(child);
            if node.kind.is_dir() && !node.is_loaded() {
                let path = node.path.clone();
                self.scanner.request(path, self.config.sort_order);
            }
        }
    }

    fn scan_done(&mut self, path: PathBuf, entries: io::Result<Vec<Entry>>) {
        self.scanner.finished(&path);

        let Some(id) = self.tree.find(&path) else {
            return;
        };

        let loaded = self.tree.node(id).is_loaded();

        match entries {
            // A failed read says nothing about the contents, so a loaded directory keeps what it
            // has rather than being emptied.
            Err(err) => {
                self.message = Some(format!("{}: {err}", path.display()));
                if !loaded {
                    self.tree.graft(id, Vec::new());
                }
            }
            Ok(entries) if loaded => {
                for removed in self.tree.reconcile(id, entries) {
                    self.watcher.unwatch(&removed);
                }
            }
            Ok(entries) => self.tree.graft(id, entries),
        }

        self.watcher.watch(&path);

        if self.tree.is_open(id) {
            self.prefetch(id);
        }

        self.mark_changed();
        self.apply_restored();

        if let Some(expansion) = self.resumed_expansion(&path) {
            self.expand_subtree(id, expansion);
        }

        // Re-ranking on every arriving folder would be quadratic, so the results catch up
        // whenever the scanner runs dry, and on the next keystroke regardless.
        if self.mode == Mode::Search && self.scanner.is_idle() {
            self.refresh_matches();
        }
    }

    /// Re-reads the directories that changed on disk, and the git status when `.git` was touched.
    fn fs_change(&mut self, paths: Vec<PathBuf>) {
        let mut directories: HashSet<PathBuf> = HashSet::new();
        let mut git_touched = false;

        for path in paths {
            if is_inside_git_dir(&path) {
                git_touched = true;
                continue;
            }

            let parents = [path.parent(), Some(path.as_path())];
            for parent in parents.into_iter().flatten() {
                if self
                    .tree
                    .find(parent)
                    .is_some_and(|id| self.tree.node(id).is_loaded())
                {
                    directories.insert(parent.to_path_buf());
                }
            }
        }

        for path in directories {
            self.scanner.request(path, self.config.sort_order);
        }

        if git_touched {
            self.refresh_git();
        }
    }

    fn refresh_git(&mut self) {
        self.git_pending = true;
        git::spawn(self.root.clone(), self.events.sender());
    }

    fn git_done(&mut self, root: &Path, result: Result<GitInfo>) {
        if root != self.root {
            return;
        }
        self.git_pending = false;

        match result {
            Ok(info) => self.git = info,
            Err(err) => self.message = Some(format!("git: {err:#}")),
        }

        // A restored `changed_only` would hide everything outside a repository.
        if !self.git.is_repo() {
            self.toggles.changed_only = false;
        }

        self.mark_all_changed();
        self.apply_filter();
    }

    fn mark_changed(&mut self) {
        let Self { tree, git, .. } = self;

        tree.mark_changed(&mut |path| git.has_changes(path));
    }

    fn mark_all_changed(&mut self) {
        let Self { tree, git, .. } = self;

        tree.mark_all_changed(&mut |path| git.has_changes(path));
    }

    pub fn is_selected(&self, row: usize) -> bool {
        row == self.tree.selected_row()
    }
}

/// A running `expand_all`: the directories it is still waiting to be read, and how many entries
/// it has taken in. The walk pauses at every unread directory and resumes as each one arrives, so
/// both have to survive those pauses; neither may outlive the walk itself.
#[derive(Default)]
struct Expansion {
    waiting: HashSet<PathBuf>,
    entries: usize,
}

#[derive(Clone, Copy)]
enum Toggle {
    Hidden,
    Gitignored,
    ChangedOnly,
}

impl Toggle {
    fn needs_git(self) -> bool {
        matches!(self, Toggle::Gitignored | Toggle::ChangedOnly)
    }
}

/// The commands that still act while a prompt or an overlay is up: they walk whatever list it
/// put there and take what the cursor lands on. The rest drive the tree, which is out of reach.
fn acts_on_top(command: &Command) -> bool {
    matches!(
        command,
        Command::MoveDown | Command::MoveUp | Command::Select | Command::Dismiss
    )
}

fn is_inside_git_dir(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == ".git")
}

#[derive(Clone, Copy)]
enum YankKind {
    Absolute,
    Relative,
}
