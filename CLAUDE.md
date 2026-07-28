# bettertree

An interactive terminal file tree driven like Helix.
Binary name: `bt`. Open it in a project directory, navigate and expand/collapse folders, see git
decorations, and drive every action through a `:command` bar with fuzzy completion.

## Module map

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | CLI args, `ratatui::init`/`restore`, `run()` |
| `src/app.rs` | `App` state, event loop, `dispatch()`, modes (Normal/Command/Help) |
| `src/events.rs` | Unified `Event` enum (input, scans, git, fs changes) + input thread |
| `src/tree/` | Node arena and filters (`mod`), directory scanning and sorting (`scan`), filesystem watching (`watch`) |
| `src/git/` | Snapshot of the repo (`mod`, `status`), blob line counts (`diff`), gitignore verdicts (`ignores`) |
| `src/config/` | Config load-or-generate (`mod`), keybinds (`keys`), colours (`theme`), `default_config.toml` |
| `src/commands.rs` | `Command` registry (name, alias, description) + fuzzy search |
| `src/command_line.rs` | The `:` prompt: input and completion |
| `src/search.rs` | The `/` prompt: query input, fuzzy matching against the tree |
| `src/state.rs` | Per-project persisted state (expanded folders, toggles, selection) |
| `src/foreground.rs` | Hand this terminal to a child process and restore the TUI after it |
| `src/editor.rs` | Resolve the configured editor (or `$EDITOR`) into a command line |
| `src/opener.rs` | `:open`: the desktop launcher, and the command line of a terminal handler |
| `src/clipboard.rs` | OSC 52 copy, so yanking works over SSH |
| `src/ui/` | Status bar, tree rows, command bar, help overlay, icons |

## Core invariants

- **The arena never removes nodes.** Deleted paths are tombstoned (`alive = false`) so `NodeId`s
  stay stable and expansion state survives rescans.
- **Collapsing only clears the folder's own `expanded` flag.** Descendants keep theirs, so
  re-expanding a folder restores exactly the shape it had. Do not prune children on collapse.
- **One dispatch path.** Keybinds and the command bar both go through `App::dispatch(Command)`.
  Never implement an action in a key handler. A prompt gets every key it can take as input: only
  non-character keys reach the keymap, and only for the few commands that prompt honours.
- **`changed_only` and a running search use an override expansion set.** Neither may mutate the
  saved `expanded` flags, so leaving them restores the tree exactly. A search shows the results
  and the folders leading to them, and opens only the latter.
- **Filters affect display only.** They never unload or skip loading nodes.
- **State is per opened root.** Child directories opened separately get their own state file.
- **Never re-walk the whole arena per event.** Gitignore verdicts and change marks advance from a
  watermark; only a new git snapshot re-marks everything. Loading a large tree must stay linear.
- **The watcher must ignore `EventKind::Access`.** inotify reports opens, and bettertree reads the
  directories it watches, so a rescan would otherwise trigger the next one forever.
- **Handing the terminal to a child process has two halves, and both matter.** Call
  `Events::suspend()` first: the input thread has to be off stdin, or it and the child split the
  user's keystrokes. And do *not* leave the alternate screen, the child switches to it itself, so
  staying put keeps the shell from flashing into view.
- **A child either gets this terminal or is kept away from it, never in between.** Foreground
  children (the editor, a handler whose desktop entry says `Terminal=true`) go through
  `pending_foreground` so the event loop runs them via `foreground::run`. Everything else is
  spawned by `opener::open` with null streams *and* `setsid`: null streams alone do not stop a
  child from opening `/dev/tty` and stealing the keys.

## Code style

- **KISS.** The simplest thing that satisfies the requirement. No speculative abstraction, no
  trait for a single implementor, no generics until a second caller exists.
- **Clean code always**, not as a later pass: small functions that do one thing, descriptive
  names, shallow nesting (early returns over `else` ladders), no dead code, no commented-out code.
- **Comments only when the code cannot explain itself**: a non-obvious invariant, a subtle `gix`
  or `notify` behavior, a deliberate deviation. Never restate what a line does; if a comment is
  needed to explain *what*, rename or extract instead.
- **Blank lines separate logical ideas inside functions.** Group the steps of an operation, blank
  line between groups. A function needing many groups wants splitting.

## Workflow

Run after every change:

```
cargo fmt
cargo clippy -- -D warnings
cargo test
```

`cargo fmt` is not optional. Clippy must be clean before a step is considered done.
