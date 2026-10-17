# Companion clients and prototypes

The Companion is an optional, experimental private-network interface for
viewing and controlling Zeus sessions from another device. It is not part of
the core remote transport and it does not change the Remote Holder
architecture. Read [Companion architecture and trust model](companion-architecture.md)
first: it records the decision, the trust model, and the production gates.

## Implementation records

| Document | Status |
|----------|--------|
| [`zeus/COMPANION_GATEWAY.md`](https://github.com/nnayz/zeus/blob/main/zeus/COMPANION_GATEWAY.md) | Implementation runbook and evidence record for the gateway (#73). Not a production release claim. |
| [`zeus/COMPANION_TERMINAL.md`](https://github.com/nnayz/zeus/blob/main/zeus/COMPANION_TERMINAL.md) | Command-oriented terminal v1 contract (#75): read-only snapshots plus explicit controller takeover. |

## Code

| Piece | Location | Notes |
|-------|----------|-------|
| External v1 DTOs | `zeus/crates/zeus-companion-api` | `API_MAJOR = 1`, `API_MINOR = 0`. Independent of the Engine wire model. |
| Gateway sidecar | `zeus/crates/zeus-companion` | Engine-launched, curated REST/JSON + WSS. Tests live in `tests/integration.rs` and `tests/security.rs`. |
| Engine integration | `zeus/crates/zeus-engine/src/control/companion.rs` | The Engine owns sessions and mutation/control fencing. |
| Reference client | `zeus/crates/zeus-companion-client` | Typed, bounded HTTP/WebSocket client. |

OpenAPI is the selected production wire and client-generation authority; until
it is published and conformance-proven, treat the Rust types and the
implementation records as the current contract.

## Browser prototype

`prototypes/companion-web/` is an isolated, dependency-free PWA prototype. The
Node server is a **synthetic loopback fixture**, never a gateway: the fixture
never connects to SSH, an Engine, or a real agent, and its routes must not be
deployed.

```sh
just prototypes start   # http://127.0.0.1:4174/ opens the pairing fixture
just prototypes test
just prototypes measure
```

`README.md` documents the gateway contract, bounds, and the privacy/curtain
behavior. [`VALIDATION.md`](https://github.com/nnayz/zeus/blob/main/prototypes/companion-web/VALIDATION.md)
records optional Chromium checks, measured findings, and known gaps.

## Native iOS client

`ios/ZeusCompanion/` is a SwiftUI client of the versioned HTTPS API. It
remains a client: it must not embed an Engine, execute SSH, own sessions, or
introduce a Swift daemon or CLI.

- Open `ios/ZeusCompanion/ZeusCompanion.xcodeproj` in Xcode 26 or newer and
  run the `ZeusCompanion` scheme on an iOS 17+ device or simulator. Signing is
  needed for a physical device. Alternatively, install
  [just](https://github.com/casey/just) 1.31 or newer and use `just ios build`
  or `just ios run` from anywhere in the repository.
- The client requires HTTPS; it stores the paired bearer token in Keychain and
  validates the server ID and API major version. See the client's
  [`README.md`](https://github.com/nnayz/zeus/blob/main/ios/ZeusCompanion/README.md).
- For local work, `just ios dev` starts an isolated test Engine, an HTTPS
  reverse proxy, and copies a fresh pairing payload to the clipboard. The
  underlying script is `ios/scripts/companion-dev.sh`.

`just ios run` picks the first booted iPhone, otherwise the newest available
one. Set `ZEUS_IOS_SIMULATOR="iPhone 17 Pro"` to choose another.

The user-facing setup and pairing flow for the app is documented once, in the
User Guide: [iOS Companion](../ios-companion.html).

## Data and trust

Companion snapshots and recent output are sensitive: they may disclose
prompts, source code, paths, and tool-emitted secrets even to a read-only
device. Follow the storage and logging rules in the
[User Guide security model](../security-model.html#companion-and-mobile-access)
and the architecture page. Device credentials, pairing payloads,
authentication responses, prompts, terminal payloads, and full environments
must never enter request logs, diagnostics, or crash reports.
