# Getting started

## Install

Taskmaster requires macOS 15 or newer. The current universal DMG is ad-hoc signed,
not Developer ID signed or notarized. Download it only from
[GitHub Releases](https://github.com/nnayz/taskmaster/releases/latest), open the DMG,
and drag Taskmaster to Applications.

For the first launch, open Finder → Applications, Control-click or right-click
Taskmaster, and choose **Open**. If macOS still blocks it, try to open Taskmaster once, then
go to System Settings → Privacy & Security → Security and choose **Open
Anyway**. The
[illustrated macOS install guide](https://nnayz.github.io/taskmaster/install/) shows
each screen in the flow.

Do not disable Gatekeeper or remove quarantine attributes globally. Homebrew
installation and in-app updates remain unavailable until a Developer ID signed
and notarized release ships. See [Updates](updates.md).

## Ten minutes to a fleet of one

1. Open Taskmaster. If no project is open, use **Open…** (`⌘O`) or **Quick Open**
   (`⌘P`) and pick a repository from the Projects & Folders section.
2. Press `⌘N`. Choose Claude Code, Codex, or whatever is installed, type the
   first prompt, and press Return. Save that setup with `⌘S` if you will run
   it again — recipes relaunch through the same spawn path. You can also press
   `⌘T` to launch your default agent in the current project with no prompt.
3. Work in the embedded terminal. The sidebar row tells you whether that agent
   is working, waiting for you, or done. You do not have to keep reading the
   PTY.
4. Press `⌘⇧D` if the panel is closed. Use **+** to open tabs.
   **Details** combines session info and artifacts. **Review** shows the git
   diff, **Files** opens named file tabs, and **Terminal** and **Browser**
   provide additional tools inside the panel.
5. Quit Taskmaster (`⌘Q`) and reopen it. The background Engine still owns the PTY.
   The session and its scrollback are still there.

If the agent you wanted is missing from the launcher, install its CLI so it
is on `PATH`, then reopen the picker. Taskmaster does not install agents for you.
`taskmaster doctor` confirms the Engine is up and whether `claude` and `codex`
are on `PATH`.

## What you are looking at

```text
┌ sidebar ┐  ┌ terminal toolbar ──────┐  ┌ panel tabs + ┐
│ project │  │                        │  │ Details      │
│ session │  │     live terminal      │  │ Review       │
│ session │  │                        │  │ Files        │
│ + New   │  │     optional shell     │  │ Terminal     │
│ account │  │                        │  │ Browser      │
└─────────┘  └────────────────────────┘  └─────────────┘
```

The sidebar is the map of the fleet. The center is the session you are in.
The inspector is how you accept the work without leaving Taskmaster.

The menu bar extra is a glance: click the Taskmaster item to see who is noisy
without bringing the window forward.

## Three habits that pay off immediately

**One worktree per agent that writes code.** Two agents in the same checkout
will fight over files. `⌘N` and MCP `spawn_agent` can both create a worktree.
See [Worktrees](worktrees.md).

**Let a lead spawn, do not type every child yourself.** A hosted Claude,
Codex, Grok, or OpenCode session already has Taskmaster MCP. Ask it to open a
reviewer, a tester, or a second implementer. Children nest under the lead
in the sidebar. See [Orchestration](orchestration.md).

**Jump to whoever needs you.** `⌘⇧J` selects the next session that is blocked
on input. `⌘⇧O` opens the session overview. Treat the rest of the fleet as
background.

Those three habits are written out as [Playbooks](playbooks/index.md), with
prerequisites, steps, and the failures that actually show up.

## Agent support, briefly

Claude Code and Codex have the richest status detection and resume. Cursor,
Grok, OpenCode, Gemini, and the rest of the catalog still get a real PTY, a
sidebar row, and (when the CLI is installed) a launch button. Missing
binaries show an install hint instead of a dead shortcut.

## Remote, when you are ready

- **SSH hosts** live under Settings → Remote. Taskmaster bootstraps a small Helper
  on the machine. No `tmux`. See [Remote hosts](remote-hosts.md).
- **First-party nodes** add accounts, usage, and laptop↔VPS handoff. See
  [Remote nodes](remote-nodes.md).

Prefer a dedicated non-admin account on the remote machine.

## Use Taskmaster from iPhone

The native iOS Companion is a separate SwiftUI target in this repository. It
connects to the Companion HTTPS gateway on the same private network; it does
not connect to the Engine or to SSH directly. See [iOS Companion](ios-companion.md)
for setup and pairing.

## Diagnostics

```sh
taskmaster doctor
```

Daemon logs and session state live under
`~/Library/Application Support/Taskmaster`. Logs may contain terminal output, paths,
or secrets printed by a process. Redact them before sharing. Email
**[hi@nasrul.info](mailto:hi@nasrul.info)** for help. See [Support](support.md)
for what to include.

## Local data and uninstalling

Quit Taskmaster. Then stop anything still holding the socket if you also want a
clean slate (`taskmaster doctor` will tell you whether an Engine is running).
Remove the app and these directories to delete local state:

```text
~/Library/Application Support/Taskmaster
~/Library/Application Support/taskmaster
~/Library/Caches/taskmaster/updates
```

Read [Privacy](privacy.md) and the [security model](security-model.md) before
using Taskmaster with sensitive repositories or remote hosts.
