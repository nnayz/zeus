# Getting started

## Install

Zeus requires macOS 15 or newer. Install the signed, notarized universal build:

```sh
brew install --cask nnayz/zeus/zeus
```

Alternatively, download the latest DMG from
[GitHub Releases](https://github.com/nnayz/zeus/releases/latest), open it, and
drag Zeus to Applications. The app checks the same release feed for updates; it
never installs one until you click restart. See [Updates](updates.md).

## Your first session

1. Open Zeus and add a project directory.
2. Create a session and choose Claude Code, Codex, another supported agent, or a
   plain shell.
3. Work in the embedded terminal. The sidebar summarizes whether each agent is
   working, waiting for input, or done.
4. Quit and reopen Zeus. The background daemon owns the PTY, so the session and
   its terminal history remain available.

Claude Code and Codex have first-class status detection and resume support.
Other agents may offer partial detection; every agent can still run as a normal
terminal.

## Parallel work with worktrees

Create separate sessions with separate git worktrees when agents may edit the
same repository. This keeps branches and working trees isolated while Zeus gives
you one place to monitor them. Treat each worktree as a normal checkout: commit
or move changes before deleting it.

## Agent orchestration

Zeus ships a CLI and MCP server. A running agent can create or inspect Zeus
sessions when you explicitly configure the MCP server. That capability is
powerful: it can launch local processes with your user privileges. Only expose
it to agents you trust and review requested actions.

## Remote execution

- **SSH hosts** — add machines under Settings → Remote. See
  [Remote hosts](remote-hosts.md).
- **First-party nodes** — optional `zeus-node` for accounts, usage, and handoff
  on a VPS. See [Remote nodes](remote-nodes.md).

Prefer a dedicated non-admin account when possible, and avoid forwarding
credentials the remote job does not need.

## Diagnostics

```sh
zeus doctor
```

Daemon logs and session state live under
`~/Library/Application Support/Zeus`. Logs may contain terminal output, paths,
or secrets printed by a process — redact them before sharing. Email
**[hi@nasrul.info](mailto:hi@nasrul.info)** for help; see [Support](support.md)
for what to include.

## Local data and uninstalling

After stopping Zeus and its daemon, remove the app and these directories if you
also want to remove its local state:

```text
~/Library/Application Support/Zeus
~/Library/Application Support/zeus
~/Library/Caches/zeus/updates
```

Read [Privacy](privacy.md) and the [security model](security-model.md) before
using Zeus with sensitive repositories or remote hosts.
