# diri

For first-party VPS execution, per-node Claude/Codex accounts, fleet usage,
and transactional local↔cloud handoff, see [NODE.md](NODE.md).

`diri` is the Rust + GPUI desktop app. The workspace holds the protocol/client core, terminal renderer, shared design system, session store, usage accounting, and window/sidebar shell. [`PLAN.md`](PLAN.md) is the historical record of the port from the retired Swift client, kept for its architecture and coexistence notes.

## Install

```sh
brew install --cask cristicretu/diri/diri
```

Or download the DMG from [the latest release](https://github.com/cristicretu/diri/releases/latest).
Either way you get the same universal build, signed and notarized, so it opens
without a Gatekeeper prompt.

The cask lives in [cristicretu/homebrew-diri](https://github.com/cristicretu/homebrew-diri)
rather than `homebrew-cask`, which requires a notability threshold the project
does not meet yet. It declares `auto_updates true`, so Homebrew installs diri
once and then leaves it alone — diri updates itself after that, and
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
cargo run -p diri-app
./scripts/build.sh
```

The app uses blurred window backing, a translucent persistent-width sidebar, an opaque Dirijor Dark terminal card, full-size content under transparent titlebar chrome, adjusted traffic lights, and a 900×560 minimum size.

### Sidebar preview fixtures

Deterministic sidebar fixtures render without connecting to the daemon. Run any scenario with:

```sh
DIRIJOR_SIDEBAR_PREVIEW=1 DIRIJOR_SIDEBAR_SCENARIO=typical cargo run -p diri-app
DIRIJOR_SIDEBAR_PREVIEW=1 DIRIJOR_SIDEBAR_SCENARIO=stress cargo run -p diri-app
DIRIJOR_SIDEBAR_PREVIEW=1 DIRIJOR_SIDEBAR_SCENARIO=empty cargo run -p diri-app
```

Preview mode uses deterministic mock dates, account identity, and usage values. It never opens a daemon connection or reads local account/transcript data.

## Remote hosts

Add, edit, or remove execution hosts from **Settings → Remote**. The catalog is
stored per installation in
`~/Library/Application Support/Dirijor/hosts.json`. `forge` is the current
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

`diri` is a client of the installed Dirijor daemon. It never replaces or automatically restarts `dirijord`; the sole explicit exception is Settings → Remote, where changing iPhone companion access asks the daemon to reload `remote.json`. Until the protocol gains multi-desktop geometry arbitration, do not focus the same session in Dirijor and `diri` at different terminal sizes; both desktop clients would resize the same PTY. Input sent from both apps is interleaved.
