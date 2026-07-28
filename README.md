# bettertree

An interactive terminal file tree driven like Helix.

Open `bt` in a project directory, navigate and expand folders, see git decorations, and drive
every action through a `:command` bar with fuzzy completion.

## Features

- Expand and collapse folders, with the shape of the tree remembered per project between runs
- Git decorations: modified, staged, untracked, ignored and conflicted entries, plus per-file
  `+n -m` diffstats and a "changed only" view
- Nerd Font file-type icons, or plain unicode arrows, or neither
- Fuzzy `/` search that narrows the tree to the matches and the folders leading to them
- A `:` command bar with fuzzy completion over every command
- Opens files in `$EDITOR` and hands the terminal over cleanly
- Yanks paths through OSC 52, so copying works over SSH
- Watches the filesystem, so the tree stays current while you work

## Install

```
cargo install bettertree
```

## Usage

```
bt [PATH]      # defaults to the current directory
```

Press `?` for the list of commands and their keys, or `:` to run one by name.

Some essentials:

| Key | Action |
| --- | --- |
| `j` / `k` | Move down / up |
| `h` | Move to the containing folder |
| `enter` | Expand or collapse a folder, or open a file in `$EDITOR` |
| `o` | Open the file in its default application |
| `e` / `c` | Expand / collapse everything under the folder |
| `.` `i` `m` | Toggle dotfiles / gitignored / changed-only |
| `/` | Search |
| `q` | Quit |

## Configuration

The first run writes a commented config to `~/.config/bettertree/config.toml` (or your
platform's equivalent) with every option at its default.
