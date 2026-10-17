# The workbench

Taskmaster is a three-pane machine. Learn the panes and the rest of the product
falls into place.

## Workspaces

The sidebar displays all workspaces and their sessions together. Select a
workspace heading or session to activate it. `⌘O` opens the chooser, where
**Open folder…** adds a workspace. Each workspace
remembers its selected session, open panel tabs, and active tab.

## Sidebar

Projects group sessions. Each row is one agent or shell: title, host badge,
and a status mark. Click a row to focus it. `⌘1` through `⌘8` jump to the
nth visible session. `⌘9` jumps to the last.

Useful moves:

- **New Agent** at the top of the sidebar, or `⌘N`, opens the composer.
  `⌘N` can also load a saved launch recipe.
- **Pin** a project or session you always want at the top.
- **Collapse** a lead session to hide its spawned children when the tree is
  loud.
- **Rename** the selected session with `⌘R`.
- **Archive** with `⌘⇧W`. Archived rows drop into the project's archive
  group. They stay as history. They are not the resume path.
- **Reorder** a row inside its project with `⌃⌘↑` and `⌃⌘↓`.
- **Needs you** is `⌘⇧J`. It walks sessions that are blocked on input.

The footer contains the account menu for usage, settings, and updates. It can
also show an **Update to …** pill. See [Updates](updates.md).

## Terminal

The center card is a real terminal, not a chat log. Copy and paste work as
usual (`⌘C` / `⌘V`). Find is `⌘F`, then `⌘G` / `⌘⇧G`. Font size is `⌘+`,
`⌘-`, and `⌘0`, or Settings → Terminal.

`⌘J` opens a second, auxiliary shell under the same session. Use it for git,
tests, or a one-off command without stealing the agent's PTY.

Spawned sessions remain grouped under their parent in the sidebar. Select any
row to open its terminal.

## Inspector

`⌘⇧D` shows or hides the panel. Use **+** to open a tab, select a tab to
switch views, and use its close button to remove it. Open tabs and the active
selection are remembered for each workspace.

| Tab | Job |
|-----|-----|
| **Details** | Session briefing, identity, attention, lifecycle, runtime, artifacts, ports, and pull requests |
| **Review** | Git workspace for this checkout: repository header, branch picker, pull-request navigation, and the live diff. Stage and commit stay on the working tree |
| **Files** | Worktree-scoped Changed / All files tree. Opening a file creates a named editor tab |
| **Terminal** | An independent shell in the current workspace; open more than one |
| **Browser** | An embedded web page with an address field and navigation controls; open more than one |

Closing a Terminal tab leaves its shell running and available in the sidebar.
Closing an edited file requires saving or reverting its changes. `⌘W` closes
the active panel tab while the panel has focus; in the main terminal it closes
the selected session. Arrow keys browse the tab strip when a tab has focus.

Review is how Taskmaster earns the "accept the work" claim. Let the agent finish,
then read the diff in the same window you used to watch it.

Files opens on **Changed**, using the same staged, working-tree, rename,
conflict, and untracked status data as Review. Switch to **All files** to browse
the selected local session's repository; directories load only when expanded,
and Git-ignored entries stay out of the tree. Opening a path from Review,
Quick Open, or a terminal reference opens a named file tab. `⌘⇧E` opens Files
and focuses the tree. Type to filter, use
`↑` / `↓` to move, `→` / `←` to expand or collapse, Return to open, and Esc
or Tab to return focus to the viewer. Non-repository sessions can still use
All files, while Changed explains that Git status is unavailable. Remote
sessions remain view-only and are not browsed through the local filesystem.

Details answers the quick briefing questions for the selected session: what kind
of agent it is, where it runs, whether it needs you, when it last completed a
turn, whether a remote process can survive disconnect, and how it relates to
parent, child, and `⌘J` workbench sessions. Sleeping and ended sessions explain
their lifecycle state and resumability. Paths, timestamps, and session IDs can
be copied from the panel.

Details keeps Git to a one-shot working-tree summary that jumps to Review; it does
not poll in the background, show pull-request content, or mutate the repository.
Artifacts and listening ports appear in the same Details tab. Installation-
level usage remains in the account menu at the bottom of the sidebar; Details
only shows usage when Taskmaster can attribute it to the selected session.

## Session overview

`⌘⇧O` (also **Session Overview** in the palette) is the board of every live
session. Use it when the sidebar is a long list and you want a spatial
scan. Click a card to jump.

## History

`⌘⇧H` opens recent activity. It is the short-term memory of what this
installation has been doing, not a search engine over every byte of
scrollback.

## Worktrees sheet

`⌥⌘W` lists git worktrees Taskmaster knows about, which session owns which, and
which look stale enough to clean up. Cleanup is suggest-only: dirty,
unmerged, or main-branch trees will not reach the confirm step. See
[Worktrees](worktrees.md).

## Menu bar

The Taskmaster extra in the macOS menu bar is a quiet radar. It shows how many
sessions need you without stealing focus. Click it to pick a row and come
back to the window.

## Command palette and Quick Open

`⌘K` is the index of everything Taskmaster can start: new agents, new shells,
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
