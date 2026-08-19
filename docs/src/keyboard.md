# Keyboard

Zeus is built to be driven without hunting through menus. The palette (`⌘K`)
is the backup for anything you forget.

## Start and stop

| Shortcut | Action |
|----------|--------|
| `⌘N` | New Agent composer (pick kind, folder, first prompt) |
| `⌘T` | New session with your default agent |
| `⌥⌘T` | New terminal (login shell) |
| `⌘⇧N` | New Codex session |
| `⌘O` | Open a project folder |
| `⌘P` | Quick Open a folder, then spawn the default agent |
| `⌘W` | Close the selected session |
| `⌘⇧T` | Reopen the last closed session |
| `⌘⇧W` | Archive the selected session |
| `⌘Q` | Quit the app (sessions keep running) |

## Move

| Shortcut | Action |
|----------|--------|
| `⌘1` … `⌘8` | Select the nth session |
| `⌘9` | Select the last session |
| `⌥⌘↑` `⌥⌘↓` | Previous / next session |
| `⌘[` `⌘]` | Same, wrapping through the list |
| `⌃⌘↑` `⌃⌘↓` | Reorder the selected row inside its project |
| `⌘⇧J` | Next session that needs input |
| `⌘⇧O` | Session overview |
| `⌘R` | Rename the selected session |

## Chrome

| Shortcut | Action |
|----------|--------|
| `⌘K` | Command palette |
| `⌘B` | Show or hide the sidebar |
| `⌘⇧D` | Show or hide the inspector |
| `⌘⇧H` | History |
| `⌥⌘W` | Worktrees sheet |
| `⌘,` | Settings |
| `⌘J` | Auxiliary shell under the current session |

## Terminal

| Shortcut | Action |
|----------|--------|
| `⌘F` | Find in the terminal |
| `⌘G` / `⌘⇧G` | Find next / previous |
| `⌘C` / `⌘V` | Copy / paste |
| `⌘+` `⌘-` `⌘0` | Zoom in, out, reset |

## Workflow tree

When the lineage view is **Tree**:

| Shortcut | Action |
|----------|--------|
| `↑` `↓` | Move between nodes |
| Return | Open that session's terminal |
| Double-click | Same |
| Esc | Back to tabs |

## Palette worth typing

You do not have to remember every remote or project shortcut. Open `⌘K` and
type:

- `new claude`, `new grok`, `new opencode`
- a project folder name
- a host name (`new codex on forge`)
- `workflow` for the agent tree
- `worktree`, `settings`, `update`

Unavailable agents stay searchable. If Zeus has a setup URL it will open the
install docs instead of spawning a dead session.
