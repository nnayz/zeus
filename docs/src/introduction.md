# Taskmaster

<img src="images/taskmaster-logo.svg" alt="Taskmaster" width="160">

Native macOS control plane for coding agents. Run Claude Code, Codex, Cursor,
Grok, OpenCode, Gemini, and a catalog of other CLIs in parallel, locally or
over SSH. Each session is a real PTY with a live status (working, needs you,
done). Closing the window never kills an agent. A daemon restart brings the
conversations back.

Taskmaster is not an IDE and not a model. It is the place you watch a fleet, jump
to the session that is stuck, and accept the work.

## Install

Taskmaster requires macOS 15 or newer. Download the current universal DMG from
[Releases](https://github.com/nnayz/taskmaster/releases/latest), drag Taskmaster to
Applications, then complete the one-time Gatekeeper exception. The current
release is ad-hoc signed and is not available through Homebrew.

See [Getting started](getting-started.md) or the
[illustrated macOS install guide](https://nnayz.github.io/taskmaster/install/) for
the complete installation flow.

## In this book

| Section | What it covers |
|---------|----------------|
| [Getting started](getting-started.md) | First project, first session, first quit |
| [The workbench](workbench.md) | Sidebar, panel tabs, terminal, overview |
| [Keyboard](keyboard.md) | Every shortcut that matters |
| [Agents and status](agents.md) | Catalog, resume, what the colors mean |
| [Worktrees](worktrees.md) | Isolated checkouts for parallel edits |
| [Orchestration](orchestration.md) | MCP spawn, wait, prompt, read |
| [Fleet patterns](fleet.md) | Ways to run more agents than you can watch |
| [Settings](settings.md) | Defaults, layout, hibernate, hosts |
| [Command line](cli.md) | `taskmaster session`, worktrees, doctor |
| [Remote hosts](remote-hosts.md) | SSH machines in Settings → Remote |
| [Remote nodes](remote-nodes.md) | First-party `taskmaster-node` on a VPS |
| [iOS Companion](ios-companion.md) | Pair a phone and use a session |
| [Updates](updates.md) | How auto-update works |
| [Playbooks](playbooks/index.md) | Task workflows: worktrees, delegation, review, remote, blockers, limits |
| [Security model](security-model.md) | Trust boundaries |
| [Privacy](privacy.md) | Local data and network activity |
| [Support](support.md) | Diagnostics and logs |
| [Roadmap](roadmap.md) | Product direction |

This is the user guide: how to install and operate Taskmaster, including the
playbooks. Building, testing, and architecture stay in the
[developer guide on GitHub](https://github.com/nnayz/taskmaster/blob/main/docs/dev/src/introduction.md).
They are not published on this site.
