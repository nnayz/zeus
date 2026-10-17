# Headless Linux Engine

The desktop app is macOS, but the Engine and the CLI are Rust and run headless
on Linux. A Linux box can be the execution host: sessions live in per-session
holders owned by the Engine, and you drive them from the `zeus` CLI over the
same owner-only Unix socket the Mac app uses. No `tmux`, `screen`, or `zellij`
is involved — a holder owns the PTY and the agent process tree directly.

Two ways to run sessions on Linux:

- **Watch them from the Mac app:** add the Linux machine as a
  [remote host](remote-hosts.md). The app bootstraps the packaged Linux Remote
  Helper over SSH and runs sessions under a holder on that host. This is the
  default path and needs nothing installed on the remote side.
- **Run the Engine on the Linux box (this page):** `zeusd-rs` and the CLI run
  on the machine itself. Useful for servers, build machines, and scripted
  orchestration where no desktop is present.

## Build

From the repository root:

```sh
cd zeus
cargo build --release -p zeus-engine -p zeus-cli -p zeus-mcp
```

The binaries matter as a set:

| Binary | Role |
|---|---|
| `target/release/zeusd-rs` | the Engine: session registry, orchestration, status |
| `target/release/zeus-holder` | per-session holder; must sit beside `zeusd-rs` |
| `target/release/zeus-cli` | the CLI, shipped as `zeus` in the macOS app bundle |
| `target/release/zeus-mcp` | MCP proxy for hosted agents; must sit beside the CLI |

`cargo build -p zeus-engine` builds `zeus-holder` too. Hosted agents get the
Engine's MCP injection only when `zeus-mcp` sits beside the CLI; without it the
agent falls back to the CLI itself.

The agent catalog (`manifests/`) is a build-time resource: a loose Cargo build
resolves it from the source tree, and a copied deployment should carry a
`manifests` directory beside the Engine (`ZEUS_MANIFESTS_DIR` overrides the
search). A missing catalog does not stop the Engine: any agent without a
manifest silently degrades to a bare login shell, so keep the directory
complete and never ship a shrunken one.

## Run

```sh
./target/release/zeusd-rs
```

The Engine stores all state in one directory: `state.json`, `daemon.sock`,
`daemon.lock`, `logs/`, `holders/`, and the installed CLI helpers.

| Platform | Default state root |
|---|---|
| Linux | `$XDG_STATE_HOME/zeus` (default `~/.local/state/zeus`) |
| macOS | `~/Library/Application Support/Zeus` |

`ZEUS_APP_SUPPORT` overrides the root for both the Engine and the CLI, which
makes secondary engines and test fleets safe: nothing in a run with the
override set can touch the logged-in account's sessions.

The Engine is a normal foreground process. Run it under whatever supervision
you already use, and stop it with a signal — closing a client never ends
sessions, because holders outlive both the CLI and the Engine.

## Drive it from the CLI

```sh
zeus --help                              # the full map

zeus session list
zeus session spawn shell --cwd ~/src/project   # prints "<id>  <label>  <title>"
zeus session read <id> --source screen
zeus session send <id> "run the tests again"
zeus session wait <id> --until done --timeout 600
zeus session release <id> --remove       # kill the tree and drop the row
```

Spawn kinds are catalog ids (`claude-code`, `codex`, `cursor`, `shell`, …).
Use `--json` on session, worktree, events, and artifacts commands to script.
`zeus doctor` checks that the Engine is reachable.

Because holders own the sessions, the CLI is stateless. A restart of
`zeusd-rs` — including a crash — adopts every live holder: the session keeps
its id, its record, and its terminal screen, and input continues where it
stopped. This is the same session identity, status reduction, and persistence
behavior as the local macOS Engine, since it is the same Engine code.

## What Linux does not have

- No desktop app: GPUI and the window shell remain macOS-only, and no GUI is
  required to run the Engine.
- Screenshot capture (`screenshot` MCP tool) is macOS-only and returns a
  structured unsupported error elsewhere.
- The Mac app does not drive a remote Engine; from the Mac, use the remote
  host path above.

## Verify

`zeus/scripts/headless-cockpit-smoke.sh` boots an Engine with a private state
root, spawns a shell through the real CLI, delivers input, reads the screen
back, kills the Engine, and asserts a restart adopts the same session with its
screen intact — with no `tmux` reachable on `PATH`. It runs on Linux and macOS;
CI runs it in the **Headless Engine + CLI (Linux)** job.
