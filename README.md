<h1><img src="docs/src/images/zeus-pixel-art.png" alt="Zeus" width="300"></h1>

[![CI](https://github.com/nnayz/zeus/actions/workflows/ci.yml/badge.svg)](https://github.com/nnayz/zeus/actions/workflows/ci.yml)
[![GitHub release](https://img.shields.io/github/v/release/nnayz/zeus)](https://github.com/nnayz/zeus/releases/latest)

Native macOS orchestrator for coding agents. Run Claude Code, Codex, Cursor, Grok, OpenCode,
Gemini and plain shells in parallel, across git worktrees or on remote hosts, each with a live
status (working / needs-you / done). Closing the app never kills a session. A daemon restart
brings conversations back.

![Zeus desktop app preview](site/static/preview.png)

## Install

```sh
brew tap nnayz/zeus https://github.com/nnayz/zeus.git
brew install --cask nnayz/zeus/zeus
```

Or download the latest DMG from [Releases](https://github.com/nnayz/zeus/releases/latest),
open it, and drag Zeus to Applications. Either way it is the same universal build (Apple
silicon and Intel), signed and notarized. Zeus updates itself from there.

Zeus keeps its Homebrew cask in this monorepo's root [`Casks`](Casks/)
directory. The explicit tap URL tells Homebrew to use this repository as the
`nnayz/zeus` tap.

macOS 15 or newer.

## 60-second tour

1. Open a project (`⌘O` or `⌘P`) and press `⌘N` (or `⌘T` for the default agent).
2. Run several sessions. Give each writer its own git worktree so they do not
   collide. Ask a hosted agent to `spawn_agent` rather than opening every child
   by hand.
3. Watch the sidebar instead of every terminal. `⌘⇧J` jumps to whoever needs
   you. The workflow tree (`⌘K` → Agent Workflow Tree) is the orchestra pit.
4. Accept the work in the inspector Review tab. Quit and reopen Zeus: the
   daemon still holds every PTY.

The [docs book](https://docs.zeus.nasrul.info) is the user manual: workbench, keyboard,
agents, orchestration, fleet patterns, remotes, and uninstalling. Build it locally with
`mdbook serve docs`.

## What it does

- **Many agents at once.** Each session is a real terminal with a real PTY. Group them by
  project, split them across git worktrees, or run them on a remote host over SSH.
- **Status you can trust.** zeus reads what an agent actually painted on its screen and tells
  you which ones are working, which are waiting on you, and which are done, so you can watch
  ten sessions without reading ten terminals.
- **Sessions outlive the app.** A background daemon owns the PTYs. Quit zeus, reopen it, and
  everything is still there.
- **Agents can orchestrate agents.** An MCP server lets a running agent spawn another one,
  watch it, read its output, and answer its prompts.

Claude Code and Codex get the richest status detection and resume. Cursor, Grok, OpenCode,
Gemini, and the rest of the catalog still get a real PTY and a sidebar row. Anything without a
manifest runs as a terminal with running/exited status.

## Architecture

Two processes, one wire protocol:

- **`zeus`**: the desktop app, Rust + [GPUI](https://github.com/zed-industries/zed). Owns the
  window, sidebar, terminal renderer, command palette, and usage accounting. Lives in
  [`zeus/`](zeus/).
- **`zeusd-rs`**: the local Engine, launched by the app and outliving it. Owns PTYs and
  child agent processes, session registry and persistence, worktrees, and the control socket.

`zeus` is also the CLI (a separate binary in `Resources/bin`): the MCP shim injected into agents,
the hook and notify forwarders, and `status`/`doctor`. `zeus-holder` owns the PTY master so
sessions survive an Engine restart.

## Agent manifests

Agent support is data, not code. Each agent is one JSON file in
`zeus/crates/zeus-engine/manifests/` describing how to spawn it, how to resume, which keys
approve or deny a prompt, and the screen rules that decide whether it is working, waiting, or
done.

## Building from source

Needs Rust (pinned in `zeus/rust-toolchain.toml`). The first build compiles GPUI from a pinned
Zed revision and takes a while.

```sh
(cd zeus && cargo build)                   # workspace
(cd zeus && cargo run -p zeus-app)         # run the app from source

zeus/scripts/package.sh                    # full bundle
zeus/scripts/install-local.sh
```

Run the same core checks as CI with one command:

```sh
./scripts/check.sh
```

User docs are the [mdBook under `docs/`](docs/). Packaging and release notes:
[`zeus/PACKAGING.md`](zeus/PACKAGING.md),
[`zeus/UPDATING.md`](zeus/UPDATING.md).

## Support

Email **[hi@nasrul.info](mailto:hi@nasrul.info)** for help, bugs, feedback, or
security reports. See also [support](docs/src/support.md),
[privacy](docs/src/privacy.md), [security](SECURITY.md), and the
[roadmap](docs/src/roadmap.md).
