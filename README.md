# zeus

Native macOS orchestrator for coding agents. Run Claude Code, Codex, Cursor, Gemini, and
plain shells in parallel — across git worktrees or on remote hosts — each with a live status
(working / needs-you / done), tmux-like persistence (closing the app never kills a session; a
daemon restart brings conversations back), a menu-bar rollup, and an MCP server so agents can
spawn and orchestrate other agents.

## Architecture

Two processes, one wire protocol:

- **`zeus`** — the desktop app: Rust + [GPUI](https://github.com/zed-industries/zed). Owns the
  window, sidebar, terminal renderer, command palette, and usage accounting. Lives in
  [`zeus/`](zeus/).
- **`zeusd`** — headless Swift daemon, launched by the app and outliving it. Owns PTYs and
  child agent processes, an offset-addressed output log per session (for detach/replay), a
  headless terminal emulator for status detection, the session registry and persistence,
  worktrees, and the control socket.
- **`zeus`** — small CLI: `mcp-stdio` (the MCP shim injected into agents), `hook`/`notify`
  (fail-open forwarders wired into Claude hooks and Codex notify), and `status`/`doctor`.

`zeusd-holder` is a tiny helper that owns the PTY master so sessions survive a daemon
crash or restart.

### Swift packages

| Target | Role |
|---|---|
| `ZeusCore` | Domain models (SessionRecord, status, attention, titles) and agent manifests |
| `ZeusProtocol` | Control-channel NDJSON envelope + binary data-channel frame codec |
| `ZeusClient` | `DaemonClient` + `SessionAttachment` actors |
| `ZeusDetection` | JSON manifest engine + `StatusReducer` state machine |
| `ZeusGit` / `ZeusMCP` / `ZeusDaemonKit` | Worktrees, MCP server, PTY/log/registry/IPC |
| `ZeusHolderKit` / `CZeusPTY` | PTY ownership that outlives the daemon |

## Build & run

The app needs both toolchains: Rust 1.95 (pinned in `zeus/rust-toolchain.toml`) and Swift 6
with the Xcode command-line tools.

```sh
# Engine
swift build
swift test

# App
cd zeus
cargo build
cargo run -p zeus-app

# Full bundle (builds the daemon, holder, and CLI into zeus.app)
zeus/scripts/package.sh
zeus/scripts/install-local.sh
```

See [`zeus/PACKAGING.md`](zeus/PACKAGING.md) for signing and notarization,
[`zeus/NODE.md`](zeus/NODE.md) for running agents on a remote VPS node, and
[`zeus/PERF.md`](zeus/PERF.md) for the rendering performance gates.

## Agent support

First-class status detection and resume: Claude Code and Codex. Cursor and Gemini run with
partial support (see the manifests for what each one wires up); everything else runs as a
generic terminal with running/exited status. Detection rules are data — they live in
`Sources/ZeusCore/Resources/manifests/` and adding an agent is a JSON file, not code.

## License

Apache 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
