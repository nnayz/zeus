# Tests, fixtures, and debugging

## Default checks

From the repository root:

```sh
./scripts/check.sh
```

That is `bash -n` on the shell scripts, the release-publishing guard tests,
then in `taskmaster/`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Release handoff also runs:

```sh
(cd taskmaster && cargo build --workspace --release)
```

Prefer the narrowest package test while iterating, then the full set before
you hand off. `./scripts/check.sh --browser` adds the browser sidecar audit
and the `taskmaster-engine` browser tests. It needs npm and Playwright.

## What the suite refuses to touch

Remote tests should not need a developer's real SSH host. Use fake `ssh`
executables, fixture homes, Unix sockets, and spawned test PTYs. Cover noisy
shell startup, OS and architecture parsing, bootstrap interruption, corrupt
artifacts, detach and reconnect, slow attach, controller revocation, exit
paths, and persistence outcomes.

Any test that uses a real remote host is opt-in and must document its
environment variables and cleanup. CI builds `taskmaster-remote probe` natively
on Linux x86_64, Linux aarch64, and macOS arm64, and runs a disposable
ordinary-user OpenSSH detach/reconnect soak. Those jobs are the remote
release gates. Do not point them at a personal machine.

Protocol fixtures for the local client live under
`taskmaster/crates/taskmaster-proto/tests/fixtures/`.

## Sidebar preview

Deterministic workbench fixtures do not connect to the daemon. From `taskmaster/`:

```sh
env TASKMASTER_SIDEBAR_PREVIEW=1 TASKMASTER_SIDEBAR_SCENARIO=typical ./scripts/dev.sh
```

Scenarios include `typical`, `stress`, `empty`, and `artifacts`. Preview
mode uses mock dates and usage. It does not read provider transcripts.

## Opt-in agent smoke

An ignored test checks that a real hosted agent can delegate through Taskmaster
MCP. It needs the development build running and the selected agent
authenticated. It deletes the parent and child sessions it creates.

```sh
cd taskmaster
TASKMASTER_REAL_AGENT_SMOKE=1 TASKMASTER_REAL_AGENT_KIND=codex \
  cargo test -p taskmaster-cli --test real_agent_delegation -- --ignored --nocapture
```

`TASKMASTER_REAL_AGENT_CWD` and `TASKMASTER_REAL_AGENT_TIMEOUT_SECONDS` override the
workspace and the 300-second deadline.

## Screenshot soak test

The default Rust test suite uses a fake capturer and never requests macOS
Screen Recording permission. To exercise a real Taskmaster window deliberately, run
this from `taskmaster/` while the Taskmaster app has an open window:

```sh
TASKMASTER_SCREENSHOT_SOAK=1 cargo test -p taskmaster-engine real_window_soak_is_explicitly_opt_in
```

macOS may ask for Screen Recording permission for Taskmaster. The test captures one
720p JPEG in memory, stops the stream immediately, and writes no screenshot
file, so there is no artifact cleanup. Remove Taskmaster under **System Settings →
Privacy & Security → Screen Recording** if you also want to clear the grant.

## Benchmarks and performance notes

`taskmaster/PERF.md` records measurements and the packaged-artifact gate. Figures
from older dates are context, not proof that the current tree passes.
`taskmaster/PERF-AUDIT.md` is a historical audit. Neither file replaces
`cargo test` or the remote acceptance scripts.

Do not trade session identity, input delivery, PTY draining, terminal
accuracy, or process cleanup for a benchmark number. Remote latency gates
are specified in `taskmaster/REMOTE_PORT.md`.
