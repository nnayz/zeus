# Getting started

## Install

Zeus requires macOS 15 or newer. Install the signed, notarized universal build:

```sh
brew tap nnayz/zeus https://github.com/nnayz/zeus.git
brew install --cask nnayz/zeus/zeus
```

Alternatively, download the latest DMG from
[GitHub Releases](https://github.com/nnayz/zeus/releases/latest), open it, and
drag Zeus to Applications. The app checks the same release feed for updates. It
never installs one until you click restart. See [Updates](updates.md).

## Ten minutes to a fleet of one

1. Open Zeus. If no project is open, use **Open…** (`⌘O`) or **Quick Open**
   (`⌘P`) and pick a repository from the Projects & Folders section.
2. Press `⌘N`. Choose Claude Code, Codex, or whatever is installed, type the
   first prompt, and press Return. You can also press `⌘T` to launch your
   default agent in the current project with no prompt.
3. Work in the embedded terminal. The sidebar row tells you whether that agent
   is working, waiting for you, or done. You do not have to keep reading the
   PTY.
4. Press `⌘⇧D` if the inspector is closed. **Info** is identity and usage.
   **Review** is the git diff for this worktree. **Code** opens a file.
   **Artifacts** collects ports, PRs, and other finds.
5. Quit Zeus (`⌘Q`) and reopen it. The background Engine still owns the PTY.
   The session and its scrollback are still there.

If the agent you wanted is missing from the launcher, install its CLI so it
is on `PATH`, then reopen the picker. Zeus does not install agents for you.
`zeus doctor` confirms the Engine is up and whether `claude` and `codex`
are on `PATH`.

## What you are looking at

```text
┌ sidebar ┐  ┌ toolbar + tabs or tree ┐  ┌ inspector ┐
│ project │  │                        │  │ Info      │
│ session │  │     live terminal      │  │ Review    │
│ session │  │                        │  │ Code      │
│ + New   │  │     optional shell     │  │ Artifacts │
└─────────┘  └────────────────────────┘  └───────────┘
```

The sidebar is the map of the fleet. The center is the session you are in.
The inspector is how you accept the work without leaving Zeus.

The menu bar extra is a glance: click the Zeus item to see who is noisy
without bringing the window forward.

## Three habits that pay off immediately

**One worktree per agent that writes code.** Two agents in the same checkout
will fight over files. `⌘N` and MCP `spawn_agent` can both create a worktree.
See [Worktrees](worktrees.md).

**Let a lead spawn, do not type every child yourself.** A hosted Claude,
Codex, Grok, or OpenCode session already has Zeus MCP. Ask it to open a
reviewer, a tester, or a second implementer. Children nest under the lead
in the sidebar and in the workflow tree. See [Orchestration](orchestration.md).

**Jump to whoever needs you.** `⌘⇧J` selects the next session that is blocked
on input. `⌘⇧O` opens the session overview. Treat the rest of the fleet as
background.

## Agent support, briefly

Claude Code and Codex have the richest status detection and resume. Cursor,
Grok, OpenCode, Gemini, and the rest of the catalog still get a real PTY, a
sidebar row, and (when the CLI is installed) a launch button. Missing
binaries show an install hint instead of a dead shortcut.

## Remote, when you are ready

- **SSH hosts** live under Settings → Remote. Zeus bootstraps a small Helper
  on the machine. No `tmux`. See [Remote hosts](remote-hosts.md).
- **First-party nodes** add accounts, usage, and laptop↔VPS handoff. See
  [Remote nodes](remote-nodes.md).

Prefer a dedicated non-admin account on the remote machine.

## Diagnostics

```sh
zeus doctor
```

Daemon logs and session state live under
`~/Library/Application Support/Zeus`. Logs may contain terminal output, paths,
or secrets printed by a process. Redact them before sharing. Email
**[hi@nasrul.info](mailto:hi@nasrul.info)** for help. See [Support](support.md)
for what to include.

## Local data and uninstalling

Quit Zeus. Then stop anything still holding the socket if you also want a
clean slate (`zeus doctor` will tell you whether an Engine is running).
Remove the app and these directories to delete local state:

```text
~/Library/Application Support/Zeus
~/Library/Application Support/zeus
~/Library/Caches/zeus/updates
```

Read [Privacy](privacy.md) and the [security model](security-model.md) before
using Zeus with sensitive repositories or remote hosts.
