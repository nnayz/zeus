# Porting the engine to Rust

The goal is a cross-platform diri. The app is already portable — roughly 4% of
`diri-app` is macOS-specific and it is cfg-gated. What binds diri to macOS is
the *engine*: the Swift `dirijord` stack in `Sources/`, which owns PTYs,
sessions, detection and the control socket.

This is the record of replacing it with `crates/diri-engine`.

## Rules this port follows

1. **Additive.** The Swift daemon keeps running and serving live sessions
   throughout. Nothing in `Sources/` is modified or deleted until the Rust
   engine is a proven replacement.
2. **Formats are load-bearing.** A log, socket message or on-disk record
   written by one engine must be readable by the other, or switching strands
   whatever sessions were live at the time.
3. **Rules stay data.** Detection manifests are read from
   `Sources/DirijorCore/Resources/manifests/`, not copied. One source of truth,
   no drift.
4. **Platform gaps are named, not hidden.** Unix is implemented; Windows gaps
   sit behind a `cfg` with the specific API that fills them documented at the
   seam.

## Status

| Layer | State | Notes |
|---|---|---|
| Output log | **done** | Byte-identical format; verified against a real 31 MB log the running Swift daemon wrote |
| PTY | **done** (unix) | Signal reset, `setsid`/`TIOCSCTTY`, fd hygiene, group kill — each with a test. Windows needs ConPTY, shape documented in `pty::unsupported` |
| Detection | **done** | All 19 manifests, 81 rules, 39 patterns compile and evaluate unchanged |
| Status reducer | **done** | Anti-flicker, blocker arbitration, startup grace, subagent isolation, staleness |
| Headless emulation | **done** | `alacritty_terminal`; OSC 9;4 progress scanned by hand |
| End-to-end pipeline | **done** | Real process → PTY → emulator → manifest → reducer → needs-input |
| Session | **done** | Self-driving: polled pump, ticks while quiet, kills its child on drop |
| Registry + persistence | **done** | Reads and round-trips the real `state.json` — 30 sessions, 84 projects preserved |
| Control socket | **core done** | Handshake, spawn, list, send_text, resize, read_screen, kill over NDJSON on an owner-only socket. Unported methods answer `not_found` rather than dropping the connection |
| Agent descriptors | **done** | argv, env scrubbing, colour assertion, resume flags — all read from the manifest |
| Spawn (control + MCP) | **works** | `session.spawn` builds argv from the manifest; hook/MCP injection still missing, so a Claude session started this way is screen-detected rather than hook-driven |
| Hook + notify parsing | **done** | Claude hooks and Codex notify → signals, with identity, titles and needs-input detail |
| Git facts | **done** | Branch and linked-worktree detection by reading `.git` directly; porcelain parsing |
| Worktree operations | **done** | Create, list, remove against real git; paths canonicalized so they match what git reports |
| MCP server | **done** | JSON-RPC stdio protocol + 13 tools executing against the registry |
| Holder (session survival) | not started | The reason the Rust engine cannot replace the Swift one yet |
| History / resume | **done** | Claude and Codex transcript stores; verified against the real ones — 500 conversations in 0.9s |
| Remote hosts (ssh + tmux) | **done** | argv, reattach naming, shell quoting verified through a real shell, scp handoff |
| Swift daemon retirement | not started | Only after the above ships and is proven |

## What the risky parts turned out to be

- **Regex dialect.** Swift used `NSRegularExpression` (ICU); Rust's `regex` has
  no backreferences or lookaround. Every shipped pattern compiles unchanged —
  this was the main unknown and it is retired.
- **PTY details.** The signal-mask reset in the child is not incidental: leave
  `SIGWINCH` ignored and agents never repaint after a resize. Tested directly.
- **Emulation.** Grepping the byte stream would misread erased text as present.
  A real emulator is not optional, which is why a dependency is justified here.
- **Authority was code when it should have been data.** The port briefly
  hardcoded "claude-code" to pick the hooks-led reducer. Every manifest already
  declares `agent.statusAuthority`; reading it means a new agent gets the right
  behavior by existing as a file.

## Windows, specifically

Not attempted yet, and it is a genuine implementation task rather than a port:
ConPTY replaces the fd model, there are no process groups, and job objects take
over kill-tree duty. `pty::unsupported` lists the exact calls. Linux should work
today apart from being untested — nothing in the engine is Darwin-specific
beyond what `cfg(unix)` already covers.
