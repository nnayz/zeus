# Keyboard

Zeus is built to be driven without hunting through menus. The palette (`⌘K`)
is the backup for anything you forget.

## Start and stop

| Shortcut | Action |
|----------|--------|
| `⌘N` | New Agent composer (pick kind, folder, first prompt, or a saved recipe) |
| `⌘T` | New session with your default agent |
| `⌥⌘T` | New terminal (login shell) |
| `⌘⇧N` | New Codex session |
| `⌘O` | Open a project folder |
| `⌘P` | Search project files, agent sessions, and folders |
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

## New Agent composer

`⌘N` opens the composer in the main pane. Recipes remember the agent, project,
host, worktree policy, title, and first prompt so a repeatable setup is one
action.

| Shortcut | Action |
|----------|--------|
| `⌘S` | Save the current setup as a recipe, or update the loaded one |
| `⌘⇧S` | Save as a new recipe |
| `⌘R` | Open the recipe list |
| `⌘↵` | Launch the highlighted recipe immediately |
| `⌘D` | Duplicate the highlighted or loaded recipe |
| `⌘⌫` | Delete the highlighted or loaded recipe |
| `⌥↑` `⌥↓` | Reorder the highlighted recipe |
| Return | Load the highlighted recipe into the composer, or launch the current setup |

A recipe whose agent, project, or host is gone stays visible and cannot launch
until you repair that one field. It never silently retargets.

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
