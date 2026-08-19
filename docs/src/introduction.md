# Zeus

<img src="images/zeus-pixel-art.png" alt="Zeus" width="280">

Native macOS control plane for coding agents. Run Claude Code, Codex, Cursor,
Grok, OpenCode, Gemini, and a catalog of other CLIs in parallel, locally or
over SSH. Each session is a real PTY with a live status (working, needs you,
done). Closing the window never kills an agent. A daemon restart brings the
conversations back.

Zeus is not an IDE and not a model. It is the place you watch a fleet, jump
to the session that is stuck, and accept the work.

## Install

```sh
brew tap nnayz/zeus https://github.com/nnayz/zeus.git
brew install --cask nnayz/zeus/zeus
```

Or download the latest DMG from
[Releases](https://github.com/nnayz/zeus/releases/latest). macOS 15 or newer.

## In this book

| Section | What it covers |
|---------|----------------|
| [Getting started](getting-started.md) | First project, first session, first quit |
| [The workbench](workbench.md) | Sidebar, inspector, lineage tree, overview |
| [Keyboard](keyboard.md) | Every shortcut that matters |
| [Agents and status](agents.md) | Catalog, resume, what the colors mean |
| [Worktrees](worktrees.md) | Isolated checkouts for parallel edits |
| [Orchestration](orchestration.md) | MCP spawn, wait, prompt, read |
| [Fleet patterns](fleet.md) | Ways to run more agents than you can watch |
| [Settings](settings.md) | Defaults, layout, hibernate, hosts |
| [Command line](cli.md) | `zeus session`, worktrees, doctor |
| [Remote hosts](remote-hosts.md) | SSH machines in Settings → Remote |
| [Remote nodes](remote-nodes.md) | First-party `zeus-node` on a VPS |
| [Updates](updates.md) | How auto-update works |
| [Security model](security-model.md) | Trust boundaries |
| [Privacy](privacy.md) | Local data and network activity |
| [Support](support.md) | Diagnostics and logs |
| [Roadmap](roadmap.md) | Product direction |
