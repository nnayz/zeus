# Zeus

<img src="images/zeus-pixel-art.png" alt="Zeus" width="280">

Native macOS orchestrator for coding agents. Run Claude Code, Codex, Cursor,
Gemini, and plain shells in parallel — across git worktrees or on remote hosts —
each with a live status (working / needs-you / done). Closing the app never kills
a session; a daemon restart brings conversations back.

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
| [Getting started](getting-started.md) | First session, worktrees, MCP, diagnostics |
| [Remote hosts](remote-hosts.md) | SSH execution hosts in Settings → Remote |
| [Remote nodes](remote-nodes.md) | First-party `zeus-node` on a VPS |
| [Updates](updates.md) | How auto-update works and what you see |
| [Security model](security-model.md) | Trust boundaries |
| [Privacy](privacy.md) | Local data and network activity |
| [Support](support.md) | Diagnostics and logs |
| [Roadmap](roadmap.md) | Product direction |
