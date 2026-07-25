# bettertree

An interactive terminal file tree that feels like VSCode's explorer but is driven like Helix.
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
| `src/command_line.rs` | The `:` prompt: input, completion, history |
| `src/state.rs` | Per-project persisted state (expanded folders, toggles, selection) |
| `src/editor.rs` | Suspend TUI, run `$EDITOR`, restore |
| `src/clipboard.rs` | OSC 52 copy, so yanking works over SSH |
| `src/ui/` | Status bar, tree rows, command bar, help overlay, icons |

## Core invariants

- **The arena never removes nodes.** Deleted paths are tombstoned (`alive = false`) so `NodeId`s
  stay stable and expansion state survives rescans.
- **Collapsing only clears the folder's own `expanded` flag.** Descendants keep theirs, so
  re-expanding a folder restores exactly the shape it had. Do not prune children on collapse.
- **One dispatch path.** Keybinds and the command bar both go through `App::dispatch(Command)`.
  Never implement an action in a key handler.
- **`changed_only` uses an override expansion set.** It must never mutate the saved `expanded`
  flags, so toggling it off restores the tree exactly.
- **Filters affect display only.** They never unload or skip loading nodes.
- **State is per opened root.** Child directories opened separately get their own state file.
- **Never re-walk the whole arena per event.** Gitignore verdicts and change marks advance from a
  watermark; only a new git snapshot re-marks everything. Loading a large tree must stay linear.
- **The watcher must ignore `EventKind::Access`.** inotify reports opens, and bettertree reads the
  directories it watches, so a rescan would otherwise trigger the next one forever.

## Code style

- **KISS.** The simplest thing that satisfies the requirement. No speculative abstraction, no
  trait for a single implementor, no generics until a second caller exists.
- **Clean code always**, not as a later pass: small functions that do one thing, descriptive
  names, shallow nesting (early returns over `else` ladders), no dead code, no commented-out code.
- **Comments only when the code cannot explain itself** — a non-obvious invariant, a subtle `gix`
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
