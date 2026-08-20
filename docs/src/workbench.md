# The workbench

Zeus is a three-pane machine. Learn the panes and the rest of the product
falls into place.

## Sidebar

Projects group sessions. Each row is one agent or shell: title, host badge,
and a status mark. Click a row to focus it. `⌘1` through `⌘8` jump to the
nth visible session. `⌘9` jumps to the last.

Useful moves:

- **New Agent** at the top of the sidebar, or `⌘N`, opens the composer.
- **Pin** a project or session you always want at the top.
- **Collapse** a lead session to hide its spawned children when the tree is
  loud.
- **Rename** the selected session with `⌘R`.
- **Archive** with `⌘⇧W`. Archived rows drop into the project's archive
  group. They stay as history. They are not the resume path.
- **Reorder** a row inside its project with `⌃⌘↑` and `⌃⌘↓`.
- **Needs you** is `⌘⇧J`. It walks sessions that are blocked on input.

The footer can show an **Update to …** pill. That is the only place an
available update lives until you click it. See [Updates](updates.md).

## Terminal

The center card is a real terminal, not a chat log. Copy and paste work as
usual (`⌘C` / `⌘V`). Find is `⌘F`, then `⌘G` / `⌘⇧G`. Font size is `⌘+`,
`⌘-`, and `⌘0`, or Settings → Terminal.

`⌘J` opens a second, auxiliary shell under the same session. Use it for git,
tests, or a one-off command without stealing the agent's PTY.

When a session belongs to a spawn family (a lead plus children), a compact
tab strip sits above the terminal. Switch with the mouse or, in the tree
view, with the arrow keys.

## Tabs and the workflow tree

A family of spawned sessions can be shown two ways:

- **Tabs** keep the terminal on screen and list the family as chips.
- **Tree** replaces the terminal with the genealogy: each node is the
  agent's mark, a caption with the title and status, and rails to its
  children. A working node wears a spinner around the mark.

Open the tree from the **Tabs / Tree** control, or from the command palette
(**Agent Workflow Tree**). In the tree: `↑` `↓` move, Return or a
double-click opens that session's terminal, Esc returns to tabs.

Use tabs when you are in the conversation. Use the tree when you are
conducting.

## Inspector

`⌘⇧D` shows or hides it. Four tabs:

| Tab | Job |
|-----|-----|
| **Info** | Kind, cwd, host, timing, and usage for today / this month |
| **Review** | Live git diff for this worktree. Stage, commit, and open a PR from here when the repo allows it |
| **Code** | File viewer bound to the same tree. Word wrap is a Settings toggle |
| **Artifacts** | Ports the session opened, pull requests it mentioned, and other finds |

Review is how Zeus earns the "accept the work" claim. Let the agent finish,
then read the diff in the same window you used to watch it.

## Session overview

`⌘⇧O` (also **Session Overview** in the palette) is the board of every live
session. Use it when the sidebar is a long list and you want a spatial
scan. Click a card to jump.

## History

`⌘⇧H` opens recent activity. It is the short-term memory of what this
installation has been doing, not a search engine over every byte of
scrollback.

## Worktrees sheet

`⌥⌘W` lists git worktrees Zeus knows about, which session owns which, and
which look stale enough to clean up. Cleanup is suggest-only: dirty,
unmerged, or main-branch trees will not reach the confirm step. See
[Worktrees](worktrees.md).

## Menu bar

The Zeus extra in the macOS menu bar is a quiet radar. It shows how many
sessions need you without stealing focus. Click it to pick a row and come
back to the window.

## Command palette and Quick Open

`⌘K` is the index of everything Zeus can start: new agents, new shells,
"New Claude Code in *this project*", "New Codex on *this host*", move the
selected Claude session, sync prefs, settings, docs, updates.

`⌘P` is the unified navigator. It searches files in the selected local
session's project (or the first open project on the startup screen), every
agent session, and indexed folders. Return opens a file in Code, switches to a
session, or launches your default agent in a folder; `⌘Return` launches a
terminal for a folder result. Add extra folder search roots in Settings →
General. Remote sessions still appear, but their files are not indexed through
the local filesystem.

## Layout

Settings → General → **Projects sidebar on the right** mirrors the chrome:
sidebar on the trailing edge, inspector on the leading one. The terminal
stays in the middle either way.
