# Workspace map

Taskmaster is a monorepo. The desktop, Engine, CLI, and transport are Rust under
`taskmaster/`. The native iOS Companion is a SwiftUI client under
`ios/TaskmasterCompanion/`. Swift must stay a client of the versioned Companion
HTTPS API. It must not embed an Engine, execute SSH, own sessions, or add a
Swift daemon or CLI.

`taskmaster/REMOTE_PORT.md` is the architecture baseline for remote sessions. This
page is only the crate map.

| Path | Owns |
|------|------|
| `taskmaster/crates/taskmaster-engine` | Local session engine, PTY and holder lifecycle, status reduction, host orchestration, remote bootstrap and SSH seam. Manifests live in `manifests/`. |
| `taskmaster/crates/taskmaster-proto` | Shared data models and wire codecs. `remote_pty` is the versioned Remote Helper protocol. |
| `taskmaster/crates/taskmaster-client` | Local app-to-engine client. It does not execute SSH itself. |
| `taskmaster/crates/taskmaster-term` | GPUI terminal renderer and client-side terminal interaction. |
| `taskmaster/crates/taskmaster-app` | Desktop UI. Remote actions go through the local Engine. |
| `taskmaster/crates/taskmaster-cli` | Shipped `taskmaster` CLI: hooks, notify, MCP backend, session and worktree commands. |
| `taskmaster/crates/taskmaster-node` | Optional enhanced node mode. Not a dependency of the default SSH bootstrap path. |
| `taskmaster/crates/taskmaster-terminal-state` | Shared headless terminal parser, grid, snapshot, and diff. Used by the local Engine and the remote Holder. |
| `taskmaster/crates/taskmaster-remote` | Minimal remote Helper. It does not depend on GPUI, `taskmaster-app`, `taskmaster-client`, or `taskmaster-node`. |
| `ios/TaskmasterCompanion` | Native iOS client. See [Companion clients](companion-clients.md). |
| `prototypes/companion-web` | Synthetic Companion fixture. Not the gateway and not a shipped runtime. |

A missing agent manifest does not fail the process. That agent launches as a
bare login shell. Do not shrink the catalog. See
[Agent manifests](manifests.md).

Remote sessions use one Holder process and one Unix socket per session. SSH
(`ssh -T`) carries Helper frames. SSH does not own the agent PTY. There is
no `tmux` fallback. The deleted transport must not come back as a feature
flag or a runtime path.

The local Engine owns session records, manifests, status, projects,
worktrees, and lifecycle policy. The remote Holder owns the PTY, the agent
process tree, the current terminal grid, bounded output, exit facts, and
the controller lease.
