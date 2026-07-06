# zeus

For first-party VPS execution, per-node Claude/Codex accounts, fleet usage,
and transactional local↔cloud handoff, see [NODE.md](NODE.md).

`zeus` is the Rust + GPUI desktop app, shipped self-contained: the app bundle
carries the daemon (`zeusd`), the session holders that keep agents alive
across daemon restarts and upgrades, the `zeus` CLI, and the MCP proxy. The
workspace holds the protocol/client core, the session engine, terminal
renderer, shared design system, session store, usage accounting, and
window/sidebar shell. [`PLAN.md`](PLAN.md) is the historical record of the
port from the retired Swift client, kept for its architecture and coexistence
notes.

## Engine

Sessions are owned by *holder* processes, not the daemon: the daemon can
crash, upgrade, or be swapped out and every live agent keeps running, to be
adopted by whatever daemon starts next.

Two daemons ship in the bundle. `zeusd` (Swift) is the default.
`zeusd-rs` is the cross-platform Rust engine
([`crates/zeus-engine`](crates/zeus-engine)) — same socket, same wire
protocol, same on-disk state, same holders, so flipping between them never
loses a session. Opt a machine in with:

```sh
ZEUSD_PATH=/Applications/zeus.app/Contents/Resources/bin/zeusd-rs open -a zeus
```

[`PORT.md`](PORT.md) tracks the port layer by layer, including the remaining
gaps that keep the Swift daemon the default for now.

## Install

```sh
brew install --cask nnayz/zeus/zeus
```

Or download the DMG from [the latest release](https://github.com/nnayz/zeus/releases/latest).
Either way you get the same universal build, signed and notarized, so it opens
without a Gatekeeper prompt.

The cask lives in [nnayz/homebrew-zeus](https://github.com/nnayz/homebrew-zeus)
rather than `homebrew-cask`, which requires a notability threshold the project
does not meet yet. It declares `auto_updates true`, so Homebrew installs zeus
once and then leaves it alone — zeus updates itself after that, and
`brew upgrade` will not clobber a build the app moved itself to. See
[UPDATING.md](UPDATING.md) for how that works.

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
cargo run -p zeus-app
./scripts/build.sh
```

The app uses blurred window backing, a translucent persistent-width sidebar, an opaque Zeus Dark terminal card, full-size content under transparent titlebar chrome, adjusted traffic lights, and a 900×560 minimum size.

### Sidebar preview fixtures

Deterministic sidebar fixtures render without connecting to the daemon. Run any scenario with:

```sh
ZEUS_SIDEBAR_PREVIEW=1 ZEUS_SIDEBAR_SCENARIO=typical cargo run -p zeus-app
ZEUS_SIDEBAR_PREVIEW=1 ZEUS_SIDEBAR_SCENARIO=stress cargo run -p zeus-app
ZEUS_SIDEBAR_PREVIEW=1 ZEUS_SIDEBAR_SCENARIO=empty cargo run -p zeus-app
ZEUS_SIDEBAR_PREVIEW=1 ZEUS_SIDEBAR_SCENARIO=artifacts cargo run -p zeus-app
```

Preview mode uses deterministic mock dates, account identity, and usage values. It never opens a daemon connection or reads local account/transcript data.

## Remote hosts

Add, edit, or remove execution hosts from **Settings → Remote**. The catalog is
stored per installation in
`~/Library/Application Support/Zeus/hosts.json`. `forge` is the current
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
Removing the file leaves the app in local-only mode.

## Coexistence

`zeus` launches the daemon bundled beside it when none is running, and
otherwise talks to whichever daemon owns the socket — it never kills or
automatically restarts a live `zeusd`; the sole explicit exception is
Settings → Remote, where changing iPhone companion access asks the daemon to
reload `remote.json`. Until the protocol gains multi-desktop geometry
arbitration, do not focus the same session in two desktop clients at different
terminal sizes; both would resize the same PTY, and input sent from both is
interleaved.
