# diri

Native macOS orchestrator for coding agents. Run Claude Code, Codex, Cursor, Gemini, and
plain shells in parallel — across git worktrees or on remote hosts — each with a live status
(working / needs-you / done), tmux-like persistence (closing the app never kills a session; a
daemon restart brings conversations back), a menu-bar rollup, and an MCP server so agents can
spawn and orchestrate other agents.

## Architecture

Two processes, one wire protocol:

- **`diri`** — the desktop app: Rust + [GPUI](https://github.com/zed-industries/zed). Owns the
  window, sidebar, terminal renderer, command palette, and usage accounting. Lives in
  [`diri/`](diri/).
- **`dirijord`** — headless Swift daemon, launched by the app and outliving it. Owns PTYs and
  child agent processes, an offset-addressed output log per session (for detach/replay), a
  headless terminal emulator for status detection, the session registry and persistence,
  worktrees, and the control socket.
- **`dirijor`** — small CLI: `mcp-stdio` (the MCP shim injected into agents), `hook`/`notify`
  (fail-open forwarders wired into Claude hooks and Codex notify), and `status`/`doctor`.

`dirijord-holder` is a tiny helper that owns the PTY master so sessions survive a daemon
crash or restart.

### Swift packages

| Target | Role |
|---|---|
| `DirijorCore` | Domain models (SessionRecord, status, attention, titles) and agent manifests |
| `DirijorProtocol` | Control-channel NDJSON envelope + binary data-channel frame codec |
| `DirijorClient` | `DaemonClient` + `SessionAttachment` actors |
| `DirijorDetection` | JSON manifest engine + `StatusReducer` state machine |
| `DirijorGit` / `DirijorMCP` / `DirijorDaemonKit` | Worktrees, MCP server, PTY/log/registry/IPC |
| `DirijorHolderKit` / `CDirijorPTY` | PTY ownership that outlives the daemon |

## Build & run

The app needs both toolchains: Rust 1.95 (pinned in `diri/rust-toolchain.toml`) and Swift 6
with the Xcode command-line tools.

```sh
# Engine
swift build
swift test

# App
cd diri
cargo build
cargo run -p diri-app

# Full bundle (builds the daemon, holder, and CLI into diri.app)
diri/scripts/package.sh
diri/scripts/install-local.sh
```

See [`diri/PACKAGING.md`](diri/PACKAGING.md) for signing and notarization,
[`diri/NODE.md`](diri/NODE.md) for running agents on a remote VPS node, and
[`diri/PERF.md`](diri/PERF.md) for the rendering performance gates.

## Agent support

First-class status detection and resume: Claude Code and Codex. Cursor and Gemini run with
partial support (see the manifests for what each one wires up); everything else runs as a
generic terminal with running/exited status. Detection rules are data — they live in
`Sources/DirijorCore/Resources/manifests/` and adding an agent is a JSON file, not code.

## License

Apache 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
