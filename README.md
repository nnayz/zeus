<h1><img src="docs/src/images/zeus-pixel-art.png" alt="Zeus" width="300"></h1>

[![CI](https://github.com/nnayz/zeus/actions/workflows/ci.yml/badge.svg)](https://github.com/nnayz/zeus/actions/workflows/ci.yml)
[![GitHub release](https://img.shields.io/github/v/release/nnayz/zeus)](https://github.com/nnayz/zeus/releases/latest)

Native macOS orchestrator for coding agents. Run Claude Code, Codex, Cursor, Gemini and plain
shells in parallel — across git worktrees or on remote hosts — each with a live status
(working / needs-you / done) and tmux-like persistence: closing the app never kills a session,
and a daemon restart brings conversations back.

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

1. Add a project directory and create a session for Claude Code, Codex, another
   supported agent, or a plain shell.
2. Start several sessions, ideally in separate git worktrees when they edit the
   same repository.
3. Watch the sidebar instead of every terminal: it shows which agents are
   working, waiting for you, or done.
4. Quit and reopen zeus. The daemon keeps each PTY alive and replays the session
   when you return.

The [docs book](docs/) covers getting started, remote hosts, MCP orchestration,
diagnostics, local data, and uninstalling (`mdbook serve docs`).

## What it does

- **Many agents at once.** Each session is a real terminal with a real PTY. Group them by
  project, split them across git worktrees, or run them on a remote host over SSH.
- **Status you can trust.** zeus reads what an agent actually painted on its screen and tells
  you which ones are working, which are waiting on you, and which are done — so you can watch
  ten sessions without reading ten terminals.
- **Sessions outlive the app.** A background daemon owns the PTYs. Quit zeus, reopen it, and
  everything is still there.
- **Agents can orchestrate agents.** An MCP server lets a running agent spawn another one,
  watch it, read its output, and answer its prompts.

Claude Code and Codex get first-class status detection and resume. Cursor and Gemini run with
partial support, and anything else runs as a terminal with running/exited status.

## Architecture

Two processes, one wire protocol:

- **`zeus`** — the desktop app: Rust + [GPUI](https://github.com/zed-industries/zed). Owns the
  window, sidebar, terminal renderer, command palette, and usage accounting. Lives in
  [`zeus/`](zeus/).
- **`zeusd`** — a headless Swift daemon, launched by the app and outliving it. Owns PTYs and
  child agent processes, an offset-addressed output log per session (for detach and replay), a
  headless terminal emulator for status detection, the session registry and persistence,
  worktrees, and the control socket.

`zeus` is also the CLI: the MCP shim injected into agents, the hook and notify forwarders, and
`status`/`doctor`. `zeusd-holder` owns the PTY master so sessions survive a daemon restart.

> **A Rust port of the engine is in progress** in `zeus/crates/zeus-engine`, so that zeus can run
> on Linux and Windows. It is not shipped — the released app runs the Swift daemon above. See
> [`zeus/PORT.md`](zeus/PORT.md) for what is done and what is left.

## Agent manifests

Agent support is data, not code. Each agent is one JSON file in
`Sources/ZeusCore/Resources/manifests/` describing how to spawn it, how to resume, which keys
approve or deny a prompt, and the screen rules that decide whether it is working, waiting, or
done.

## Building from source

Needs both toolchains: Rust (pinned in `zeus/rust-toolchain.toml`) and Swift 6 with the Xcode
command-line tools. The first Rust build compiles GPUI from a pinned Zed revision and takes a
while.

```sh
swift build && swift test                  # engine
(cd zeus && cargo build)                   # app
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
