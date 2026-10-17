# Build and run

All Rust commands run from `zeus/`.

## The quick loop

```sh
cargo run -p zeus-app
```

This launches the workbench against the normal Engine socket. It is the
fastest iteration loop for Rust changes, but it runs directly from
`target/`, so macOS grants (Files & Folders, Screen Recording) are tied to
whatever binary was launched.

## The development bundle

```sh
./scripts/dev.sh
```

`dev.sh` assembles a throwaway app bundle with a commit-specific name, window
title, Dock icon, and in-window build marker. The bundle identifier is
deliberately stable so macOS permissions survive rebuilds, and the Engine,
holders, and CLI helpers run from inside that signed bundle so they inherit
the selected-folder identity. The script also removes agent-session
environment variables that would stop Zeus from finding or launching the
shared daemon.

Useful flags:

```sh
./scripts/dev.sh --release
./scripts/dev.sh --settings remote
./scripts/dev.sh -- --features audio-playback
```

For projects under Desktop or Documents, set `ZEUS_DEV_SIGN_IDENTITY` to a
stable signing identity, or run `../scripts/make-dev-cert.sh` once to install
the repository's local `Zeus Dev` identity. Without it `dev.sh` falls back to
ad-hoc signing and warns that macOS grants may not survive the next rebuild.

The development and installed apps deliberately share the daemon, socket,
sessions, preferences, and Application Support directory. That makes a dev
build useful against real sessions, but do not focus the same session in both
apps at different terminal sizes: each client can resize the shared PTY.

## Watch mode

Watch mode rebuilds and relaunches the app when Rust sources, assets, scripts,
or workspace configuration change. Install watchexec once:

```sh
brew install watchexec
```

```sh
./scripts/dev.sh --watch
./scripts/dev.sh --watch --settings remote --release
```

Saving triggers a rebuild and reopen; Rust changes still require
compilation. Build failures leave the watcher running, so fix the error and
save again. Stop with `Ctrl+C`. Watch mode and the preview launchers share
`scripts/dev-watch.sh`, which watches inputs explicitly so build output and
development bundles do not cause rebuild loops.

## Preview fixtures

Deterministic fixtures render the workbench and Settings without connecting to
the daemon:

```sh
./scripts/dev-ui.sh                                   # workbench preview
ZEUS_WORKSPACE_CHOOSER_PREVIEW=1 ./scripts/dev-ui.sh  # workspace chooser
ZEUS_PANEL_PREVIEW=menu ./scripts/dev-ui.sh           # panel tabs and + menu
ZEUS_PANEL_PREVIEW=browser ./scripts/dev-ui.sh        # offline embedded browser
./scripts/dev-settings.sh                             # Settings, General tab
./scripts/dev-settings.sh terminal                    # terminal settings
./scripts/dev-gallery.sh                              # shared UI component gallery
```

The launchers resolve their own workspace directory, so they also work from
the repository root as `./zeus/scripts/dev-ui.sh`. Pass `--scenario` to
`dev-ui.sh` (`typical`, `stress`, `empty`, `artifacts`), and `--release` to
any launcher. Settings tabs are `general`, `terminal`, `resources`, and
`remote`.

The sidebar preview is driven by environment variables too:

```sh
env ZEUS_SIDEBAR_PREVIEW=1 ZEUS_SIDEBAR_SCENARIO=stress ./scripts/dev.sh
```

Preview mode uses deterministic mock dates and usage values and never opens a
daemon connection or reads provider account or transcript data.

## Live agent smoke test

An ignored test verifies that a real delegation request creates a child
`SessionRecord` through a hosted agent's Zeus MCP connection. Start the
development build, make sure the selected agent is authenticated, then run:

```sh
ZEUS_REAL_AGENT_SMOKE=1 ZEUS_REAL_AGENT_KIND=codex \
  cargo test -p zeus-cli --test real_agent_delegation -- --ignored --nocapture
```

The test uses the current directory by default. Set `ZEUS_REAL_AGENT_CWD` and
`ZEUS_REAL_AGENT_TIMEOUT_SECONDS` to override its workspace and 300-second
deadline. It removes the parent and child sessions it creates.

## iOS Companion and the web prototype

The iOS app is built from `ios/ZeusCompanion/ZeusCompanion.xcodeproj` in
Xcode (see [Companion clients and prototypes](companion-clients.md)). With
just 1.31 or newer, `just ios build` builds for the simulator, `just ios run`
builds and launches it, and `just ios dev` starts a self-contained local
gateway with TLS, an isolated Engine, and a fresh pairing payload on the
clipboard.

The browser prototype under `prototypes/companion-web/` is dependency-free:

```sh
just prototypes start   # loopback synthetic fixture
just prototypes test
```

## The website and books

```sh
cd site
npm ci
npm run dev
```

Both mdBook sources build independently; see
[Writing documentation](writing-documentation.md).
