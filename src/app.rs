use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Result;
use crossterm::event::{Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind};
use ratatui::DefaultTerminal;

use crate::clipboard;
use crate::command_line::{CommandLine, Outcome};
use crate::commands::{Command, REGISTRY};
use crate::config::{Config, Toggles};
use crate::editor;
use crate::events::{Event, Events};
use crate::git::{self, GitInfo, Ignores};
use crate::state::State;
use crate::tree::scan::{Entry, Scanner};
use crate::tree::watch::Watcher;
use crate::tree::{Filter, NodeId, ROOT, Tree};
use crate::ui;

/// A stray `:expand_all` on a huge tree stops here instead of locking up the UI.
const EXPAND_ALL_LIMIT: usize = 20_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Command,
    Help,
}

pub struct App {
    pub root: PathBuf,
    pub tree: Tree,
    pub config: Config,
    pub mode: Mode,
    pub command_line: CommandLine,
    pub help_scroll: usize,
    pub scroll: usize,
    pub message: Option<String>,
    pub git: GitInfo,
    pub git_pending: bool,
    pub toggles: Toggles,
    pending_expand: Vec<PathBuf>,
    expanding: Vec<PathBuf>,
    pending_select: Option<PathBuf>,
    events: Events,
    scanner: Scanner,
    ignores: Ignores,
    watcher: Watcher,
    viewport_height: usize,
    pending_open: Option<PathBuf>,
    quit: bool,
}

impl App {
    pub fn new(root: PathBuf, config: Config) -> Self {
        let events = Events::new();
        let scanner = Scanner::new(events.sender());
        let watcher = Watcher::new(config.watch, events.sender());
        let ignores = Ignores::open(&root);

        Self {
            tree: Tree::new(root.clone()),
            toggles: config.toggles,
            root,
            config,
            mode: Mode::Normal,
            command_line: CommandLine::new(),
            help_scroll: 0,
            scroll: 0,
            message: None,
            git: GitInfo::none(),
            git_pending: false,
            pending_expand: Vec::new(),
            expanding: Vec::new(),
            pending_select: None,
            ignores,
            watcher,
            events,
            scanner,
            viewport_height: 1,
            pending_open: None,
            quit: false,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        if let Some(git_dir) = git::git_dir(&self.root) {
            self.watcher.watch(&git_dir);
        }

        self.restore();
        self.request(ROOT);
        self.apply_filter();
        self.refresh_git();

        while !self.quit {
            if self.tree.needs_ignore_classification() {
                self.ignores.classify(&mut self.tree);
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
                Event::GitDone(result) => self.git_done(*result),
                Event::FsChange(paths) => self.fs_change(paths),
            }

            if let Some(path) = self.pending_open.take() {
                self.save();

                self.events.suspend();
                let opened = editor::open(terminal, &path, &self.config.editor);
                self.events.resume();

                if let Err(err) = opened {
                    self.message = Some(format!("{err:#}"));
                }
                self.refresh_git();
            }
        }

        self.save();

        Ok(())
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
            Mode::Help => self.handle_help_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        // `:` is the way into the command bar, so it is never rebindable.
        if key.code == KeyCode::Char(':') {
            self.message = None;
            self.command_line.open();
            self.mode = Mode::Command;
            return;
        }

        if let Some(command) = self.config.keys.get(key) {
            self.dispatch(command);
        }
    }

    fn handle_command_key(&mut self, key: KeyEvent) {
        match self.command_line.handle_key(key) {
            Outcome::Continue => {}
            Outcome::Cancel => self.mode = Mode::Normal,
            Outcome::Error(message) => self.message = Some(message),
            Outcome::Run(command) => {
                self.mode = Mode::Normal;
                self.dispatch(command);
            }
        }
    }

    fn handle_help_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => self.mode = Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') => self.help_scroll += 1,
            KeyCode::Up | KeyCode::Char('k') => {
                self.help_scroll = self.help_scroll.saturating_sub(1)
            }
            _ => {}
        }

        self.help_scroll = self.help_scroll.min(REGISTRY.len().saturating_sub(1));
    }

    /// The single place where a command becomes an action; keys and the command bar both land here.
    pub fn dispatch(&mut self, command: Command) {
        self.message = None;

        match command {
            Command::MoveDown => self.tree.move_by(1),
            Command::MoveUp => self.tree.move_by(-1),
            Command::MoveNextSibling => self.tree.move_next_sibling(),
            Command::MovePrevSibling => self.tree.move_prev_sibling(),
            Command::MoveParent => self.tree.move_parent(),
            Command::MoveFirst => self.tree.select_row(0),
            Command::MoveLast => self.tree.move_last(),
            Command::ScrollDown => self.scroll_by(self.config.scroll_lines as isize),
            Command::ScrollUp => self.scroll_by(-(self.config.scroll_lines as isize)),
            Command::HalfPageDown => self.scroll_by((self.viewport_height / 2) as isize),
            Command::HalfPageUp => self.scroll_by(-((self.viewport_height / 2) as isize)),
            Command::CenterCursor => self.center_cursor(),
            Command::Select => self.select(),
            Command::ExpandAll => self.expand_all(),
            Command::CollapseAll => self.collapse_all(),
            Command::Open => self.open_selected(),
            Command::YankPath => self.yank(YankKind::Absolute),
            Command::YankRelativePath => self.yank(YankKind::Relative),
            Command::ToggleHidden => self.toggle(Toggle::Hidden),
            Command::ToggleGitignored => self.toggle(Toggle::Gitignored),
            Command::ToggleChangedOnly => self.toggle(Toggle::ChangedOnly),
            Command::Refresh => self.refresh(),
            Command::Help => self.show_help(),
            Command::Quit => self.quit = true,
        }
    }

    fn mark_changed(&mut self) {
        let Self { tree, git, .. } = self;

        tree.mark_changed(&mut |path| git.has_changes(path));
    }

    fn mark_all_changed(&mut self) {
        let Self { tree, git, .. } = self;

        tree.mark_all_changed(&mut |path| git.has_changes(path));
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

        if self.toggles.changed_only {
            self.load_changed_directories();
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

    fn git_done(&mut self, result: Result<GitInfo>) {
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

    fn show_help(&mut self) {
        self.help_scroll = 0;
        self.mode = Mode::Help;
    }

    fn select(&mut self) {
        let id = self.tree.selected();
        let node = self.tree.node(id);

        if !node.kind.is_dir() {
            self.open_selected();
            return;
        }

        match node.expanded {
            true => self.collapse(id),
            false => self.expand(id),
        }
    }

    fn collapse(&mut self, id: NodeId) {
        self.expanding.clear();
        self.tree.set_expanded(id, false);
    }

    fn open_selected(&mut self) {
        let node = self.tree.node(self.tree.selected());

        if node.kind.is_dir() {
            self.message = Some("not a file".to_owned());
            return;
        }

        self.pending_open = Some(node.path.clone());
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

    fn expand_all(&mut self) {
        let Some(id) = self.focused_folder() else {
            return;
        };

        self.expanding.push(self.tree.node(id).path.clone());
        self.expand_subtree(id);
    }

    /// Expands everything currently known below `id`. Directories that have not been read yet
    /// stop the walk; it resumes from them in `scan_done`, so the subtree opens as it loads.
    fn expand_subtree(&mut self, id: NodeId) {
        let mut stack = vec![id];

        while let Some(current) = stack.pop() {
            if self.tree.node_count() > EXPAND_ALL_LIMIT {
                self.expanding.clear();
                self.message = Some(format!("stopped expanding at {EXPAND_ALL_LIMIT} entries"));
                return;
            }

            self.expand(current);

            let children = self.tree.node(current).children.clone().unwrap_or_default();
            let subdirectories = children
                .into_iter()
                .filter(|child| self.tree.node(*child).kind.is_dir());

            stack.extend(subdirectories);
        }
    }

    /// True while `expand_all` is still opening a subtree that contains this path.
    fn is_expanding(&self, path: &Path) -> bool {
        self.expanding.iter().any(|root| path.starts_with(root))
    }

    fn collapse_all(&mut self) {
        let Some(id) = self.focused_folder() else {
            return;
        };

        self.expanding.clear();
        self.tree.collapse_subtree(id);
        self.tree.select(id);
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

    fn expand(&mut self, id: NodeId) {
        self.tree.set_expanded(id, true);

        match self.tree.node(id).is_loaded() {
            true => self.prefetch(id),
            false => self.request(id),
        }
    }

    fn request(&mut self, id: NodeId) {
        let path = self.tree.node(id).path.clone();
        self.scanner.request(path, self.config.sort_order);
    }

    /// Loads the direct children of every subdirectory so the next expand is instant. Only
    /// expanded directories prefetch, which is what keeps this from walking the whole tree.
    fn prefetch(&mut self, id: NodeId) {
        let Some(children) = self.tree.node(id).children.clone() else {
            return;
        };

        for child in children {
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

        if self.is_expanding(&path) {
            self.expand_subtree(id);
        }
    }

    fn scroll_by(&mut self, delta: isize) {
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

    pub fn is_selected(&self, row: usize) -> bool {
        row == self.tree.selected_row()
    }
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

fn is_inside_git_dir(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == ".git")
}

#[derive(Clone, Copy)]
enum YankKind {
    Absolute,
    Relative,
}
