# taskmaster

For first-party VPS execution, per-node Claude/Codex accounts, fleet usage,
and transactional local↔cloud handoff, see
[Remote nodes](../docs/src/remote-nodes.md).

New contributors: the repository's [Developer Guide](../docs/dev/src/introduction.md)
covers setup, the crate map, and required checks. Canonical engineering
documents such as [`PACKAGING.md`](PACKAGING.md), [`UPDATING.md`](UPDATING.md),
and [`REMOTE_PORT.md`](REMOTE_PORT.md) stay authoritative here and are indexed
by the guide.

`taskmaster` is the Rust + GPUI desktop app, shipped self-contained: the app bundle
carries the Engine (`taskmasterd-rs`), the session holders that keep agents alive
across Engine restarts and upgrades, the `taskmaster` CLI, and the MCP proxy. The
workspace holds the protocol/client core, the session engine, terminal
renderer, shared design system, session store, usage accounting, and
window/sidebar shell. [`PLAN.md`](PLAN.md) is the historical record of the
port from the retired Swift client.

For the proposed local closed-lid execution investigation in #70, see
[CLOSED_LID_PLAN.md](CLOSED_LID_PLAN.md). Its
[decision record](CLOSED_LID_DECISIONS.md) and
[threat model](CLOSED_LID_THREAT_MODEL.md) keep privileged work and shipment
explicitly gated. The [spike report](CLOSED_LID_SPIKE_REPORT.md) tracks completed
and blocked evidence. This is an exploration, not approval to install a helper.

## Engine

Sessions are owned by *holder* processes, not the Engine: the Engine can
crash, upgrade, or be swapped out and every live agent keeps running, to be
adopted by whatever Engine starts next.

`taskmasterd-rs` ([`crates/taskmaster-engine`](crates/taskmaster-engine)) is the only daemon. The
app refuses any other `Hello` identity.

## Install

```sh
brew tap nnayz/taskmaster https://github.com/nnayz/taskmaster.git
brew install --cask nnayz/taskmaster/taskmaster
```

Or download the DMG from [the latest release](https://github.com/nnayz/taskmaster/releases/latest).
Either way you get the same universal build, signed and notarized, so it opens
without a Gatekeeper prompt.

The cask lives in the monorepo's root [`Casks`](../Casks/) directory; the
explicit tap URL above is required because it is not a separate GitHub
repository. It declares `auto_updates true`, so Homebrew installs taskmaster once and
then leaves it alone — taskmaster updates itself after that, and `brew upgrade` will
not clobber a build the app moved itself to. See [UPDATING.md](UPDATING.md) and
the user-facing [Updates](../docs/src/updates.md) page.

## Toolchain and GPUI pin

- Rust: `1.95.0` (stable, pinned by `rust-toolchain.toml`)
- GPUI source: [`zed-industries/zed`](https://github.com/zed-industries/zed)
- GPUI revision: [`dc2a339d5d043da448a3f7ddc7c0a85c63864aad`](https://github.com/zed-industries/zed/commit/dc2a339d5d043da448a3f7ddc7c0a85c63864aad)
- Revision date: 2026-07-22

The git revision is intentionally exact. Upgrade it deliberately and update this record when doing so.

## Build and run

```sh
cargo build
cargo clippy --workspace -- -D warnings
./scripts/build.sh
```

The macOS app uses a layered Icon Composer icon with an ICNS fallback. See
[icon artwork and regeneration](assets/icon-assets/README.md) when changing it;
release and development bundles install their matching icon resources before
signing.

Run the app under development through `dev.sh`, which builds a throwaway app
bundle with a commit-specific name, window title, Dock icon, and in-window
build marker. Its bundle id is deliberately stable so macOS Files & Folders
grants survive rebuilds, and the Engine/Holder/CLI helpers run from inside that
signed bundle so they inherit the selected-folder identity. It also removes
agent-session environment variables that would otherwise stop taskmaster from
finding or launching the shared daemon:

```sh
./scripts/dev.sh
./scripts/dev.sh --watch
./scripts/dev.sh --release
./scripts/dev.sh --settings remote
./scripts/dev.sh -- --features audio-playback
```

For protected Desktop/Documents projects, set `TASKMASTER_DEV_SIGN_IDENTITY` to a
stable code-signing identity or run `../scripts/make-dev-cert.sh` once to
install the repository's local `Taskmaster Dev` identity. `dev.sh` falls back to
ad-hoc signing but warns that macOS grants may not survive the next rebuild.

The dev and installed apps deliberately share the daemon, socket, sessions,
preferences, and Application Support directory. That makes the dev build useful
against real sessions, but do not focus the same session in both apps at
different terminal sizes: each client can resize the shared PTY.

An ignored live smoke test checks that an ordinary delegation request creates
a child `SessionRecord` through the hosted agent's Taskmaster MCP connection. Start
the current development build, ensure the selected agent is authenticated, and
run:

```sh
TASKMASTER_REAL_AGENT_SMOKE=1 TASKMASTER_REAL_AGENT_KIND=codex \
  cargo test -p taskmaster-cli --test real_agent_delegation -- --ignored --nocapture
```

The test uses the current directory by default; set `TASKMASTER_REAL_AGENT_CWD` and
`TASKMASTER_REAL_AGENT_TIMEOUT_SECONDS` to override its workspace and 300-second
deadline. It removes the parent and child sessions it creates.

The app uses blurred window backing, a translucent persistent-width sidebar, an opaque Taskmaster Dark terminal card, full-size content under transparent titlebar chrome, adjusted traffic lights, and a 900×560 minimum size.

### Sidebar preview fixtures

Deterministic fixtures render the full workbench (sidebar, terminal chrome, inspector) without connecting to the daemon. Run any scenario with:

```sh
env TASKMASTER_SIDEBAR_PREVIEW=1 TASKMASTER_SIDEBAR_SCENARIO=typical ./scripts/dev.sh
env TASKMASTER_SIDEBAR_PREVIEW=1 TASKMASTER_SIDEBAR_SCENARIO=stress ./scripts/dev.sh
env TASKMASTER_SIDEBAR_PREVIEW=1 TASKMASTER_SIDEBAR_SCENARIO=empty ./scripts/dev.sh
env TASKMASTER_SIDEBAR_PREVIEW=1 TASKMASTER_SIDEBAR_SCENARIO=artifacts ./scripts/dev.sh
```

Preview mode uses deterministic mock dates and usage values, with the profile name derived from local machine identity. It never opens a daemon connection or reads provider account/transcript data.

The sidebar displays all workspaces together. Select a workspace heading or use `⌘O` to open one. Each
workspace remembers its selected session, open panel tabs, and active tab. The profile menu
lives at the bottom of the sidebar.

### Watch UI changes

Install [Watchexec](https://github.com/watchexec/watchexec) once with
`brew install watchexec`, then use these launchers from `taskmaster/`:

```sh
./scripts/dev.sh --watch             # live app, rebuild and restart on save
./scripts/dev.sh --watch --settings remote --release
./scripts/dev-ui.sh                  # workbench preview
TASKMASTER_WORKSPACE_CHOOSER_PREVIEW=1 ./scripts/dev-ui.sh  # workspace chooser
TASKMASTER_PANEL_PREVIEW=menu ./scripts/dev-ui.sh           # dynamic tabs and + menu
TASKMASTER_PANEL_PREVIEW=browser ./scripts/dev-ui.sh        # offline embedded browser
./scripts/dev-settings.sh            # Settings, General tab
./scripts/dev-settings.sh terminal   # terminal settings
./scripts/dev-gallery.sh             # shared UI component gallery
```

The launchers resolve their own workspace directory, so they also work from
the repository root as `./taskmaster/scripts/dev-ui.sh`, for example. They watch Rust
sources, assets, scripts, and workspace configuration. Saving rebuilds and
reopens the app or preview; Rust changes still require compilation. Build failures
leave the watcher running, so fix the error and save again. Stop with Ctrl+C.

Workbench and Settings previews use the existing deterministic fixtures without
connecting to the Engine. Choose another workbench scenario with
`./scripts/dev-ui.sh --scenario stress` (`typical`, `stress`, `empty`, and
`artifacts` are supported). Settings tabs are `general`, `terminal`, `resources`,
and `remote`. Dev arguments such as `--release` can be passed through:
`./scripts/dev-ui.sh --release` or `./scripts/dev-settings.sh general --release`.

Watch mode and the preview launchers share `scripts/dev-watch.sh`, which watches
inputs explicitly so build output and development app bundles do not cause
rebuild loops.

## Remote hosts

Add, edit, or remove execution hosts from **Settings → Remote**. The catalog is
stored per installation in
`~/Library/Application Support/Taskmaster/hosts.json`. `forge` is the current
shared host, not a built-in server type or reserved id. Each installation can
use its own SSH user and can add any number of other SSH-reachable machines:

```json
{
  "hosts": [
    {
      "id": "forge",
      "name": "Forge",
      "ssh": "you@forge",
      "defaultCwd": "~/code"
    },
    {
      "id": "studio",
      "name": "Studio Mac",
      "ssh": "studio.local",
      "defaultCwd": "~/Developer"
    }
  ]
}
```

`id` is the stable value persisted with sessions, `name` is presentation only,
and `ssh` accepts either an SSH destination or an alias from `~/.ssh/config`.
Removing the file leaves the app in local-only mode. Tailscale IPv4 addresses
and MagicDNS names work like any other SSH destination when OpenSSH can resolve
them; Taskmaster neither requires nor configures Tailscale for Remote Holder sessions.

## Agent preferences

The default agent is stored in
`~/Library/Application Support/taskmaster/prefs.json` as the agent manifest's stable
`id`. Preferences written before the manifest catalog (`claudeCode`, `codex`,
`cursor`, or `gemini`) remain readable and are rewritten with the canonical id
on the next save. If a saved agent is removed or is no longer available, Taskmaster
chooses the first installed first-class agent; when none are installed it falls
back to the terminal shell.

## Coexistence

`taskmaster` launches the daemon bundled beside it when none is running, and
otherwise talks to whichever Rust Engine owns the socket. A packaged Engine
upgrade may replace an outdated daemon after holders have preserved their
sessions. Until the protocol gains multi-desktop geometry arbitration, do not
focus the same session in two desktop clients at different terminal sizes;
both would resize the same PTY, and input sent from both is interleaved.
