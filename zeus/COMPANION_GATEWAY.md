# Experimental companion gateway (#73)

The Rust gateway is an opt-in Engine child that exposes bounded session
projections and commands through `zeus-client`. The Engine owns session records,
controller leases, mutation fencing and PTY lifecycle. The gateway owns device
credentials, HTTP admission and a bounded invalidation ring; it has no session
registry or Holder connection. This implementation does not change the Remote
PTY Holder transport described in [REMOTE_PORT.md](REMOTE_PORT.md).

This is an implementation runbook and evidence record, not a production release
claim. The independent external v1 DTO authority is
[`zeus-companion-api/src/lib.rs`](crates/zeus-companion-api/src/lib.rs).
The internal `zeus-proto::terminal` RLE snapshot and RPC schema are not exposed
over HTTP. The gateway decodes and validates snapshots into plain text,
dimensions, cursor, incarnation, sequence and controller ownership facts.

## Implemented contract

All paths below are relative to the enrolled origin. JSON field names use
snake_case. Responses are uncached. Ordinary requests authenticate with
`Authorization: Bearer <device token>`; tokens are never URL parameters.

| Method and path | Required scope | Result |
| --- | --- | --- |
| `POST /v1/pair` | One-time enrollment code | `PairResponse` |
| `GET /v1/hello` | `read` | `Hello`, including stable server identity and Engine epoch |
| `GET /v1/projects` | `read` | `Page<Project>` |
| `GET /v1/sessions` | `read` | `Page<Session>` |
| `GET /v1/sessions/{id}` | `read` | `SessionDetail` |
| `GET /v1/sessions/{id}/screen` | `read` | Full bounded `Screen` |
| `POST /v1/sessions/{id}/control/acquire` | `interact` | `ControlState` |
| `POST /v1/sessions/{id}/control/release` | `interact` | `ControlState` |
| `POST /v1/sessions/{id}/text` | `interact` | `ControlState` |
| `POST /v1/sessions/{id}/actions` | `lifecycle` | `MutationResult` |
| `POST /v1/devices/{id}/revoke` with `{}` | `read`, own device only | `{}` |
| `GET /v1/events` | First-frame `read` authentication | WebSocket `Event` stream |

The advertised capabilities are exactly `projects`, `sessions`, `screen`,
`events`, `rename`, `lifecycle`, `control_lease`, and `send_text`. There is no
external spawn, scrollback, raw resize, signal, terminal diff, generic RPC,
transcript, provider payload or internal diagnostic endpoint. The reserved
`spawn` scope enum alone does not enable a capability.

Pair requests must contain `api_major: 1`, `expected_server_id`, `code` and
`device_name`. The server checks the expected identity before consuming the
code. Pairing and authenticated hello both return the same stable `server_id`.
The client must validate the enrolled origin's TLS identity and compare the
server IDs; the ID does not replace TLS authentication. Hello is authenticated,
so it is not a pre-pair server discovery service.

The WebSocket's first text frame is `Subscribe { api_major, token, cursor }`
within five seconds. `cursor` is null or `{ stream_id, sequence }`. Events are
only `changed`, `resync_required`, or `engine_unavailable` plus a cursor. They
carry no terminal or session payload. Reconnect can replay retained
invalidations; a missing, expired, future or different-stream cursor requires a
full projection refresh. Slow subscribers receive resync or disconnect instead
of holding up PTY draining. The typed Rust reference client rejects unexpected
stream changes, sequence gaps and rewinds before advancing its cursor.

## Command and revocation semantics

Acquire and release use compare-and-swap controller epochs. The authenticated
device supplies the owner identity through the gateway; a request cannot choose
another owner. Takeover is explicit. Desktop and mobile arbitration share the
Engine lease, and each screen returns the current owner and command sequence.
Mobile commands use fixed existing PTY geometry. A remote recovery queue can
make takeover unavailable; `terminal_unavailable` is preserved as a structured
conflict, without an automatic retry.

Text requests contain the expected epoch, the strictly next `command_seq`, text
and `submit`. The Engine consumes the sequence before writing once to the PTY.
Repeated commands are rejected, including after gateway restart. The client
must not resend text after a timeout, lost response or uncertain delivery.

Actions are rename, archive, wake, hibernate and terminate. Every action carries
an Engine epoch, unique 16–64 character ID and current revision. Non-rename
actions require explicit confirmation and, for a live session, the current
device's controller lease. Validation and the lifecycle effect hold the same
Registry lock. **Archiving a live session terminates its Agent before archiving;
the confirmation must disclose potential work loss.**

The Engine consumes mutation IDs before effects, retains up to 4096 without
eviction, and fails closed when that window is full. A gateway restart preserves
this state. A new Engine incarnation rejects old Engine epochs. There is no
durable outcome journal or cached success replay. `outcome_unknown` and
`input_unconfirmed` remain distinct external errors. A lost response, disconnect
or timeout can leave an effect applied; refresh and explicit user review are
required before another action. Neither reference client reconnect nor event
replay redispatches commands.

Mutation credentials are checked at admission and again after body decoding,
immediately before IPC dispatch. Revocation committed before that second check
rejects the mutation. There is no atomic transaction between the gateway auth
file and Engine execution: revocation after that check cannot recall an already
dispatched or queued Engine command. Screen responses and WebSocket delivery
also recheck auth; an idle WebSocket checks at least once per second. Bytes
already handed to the socket cannot be recalled. Other metadata reads check at
admission and may finish after revocation. Revocation does not release an
Engine controller lease; desktop takeover or an authorized device provides
recovery.

## Local operation

Build from `zeus/` using the repository toolchain. The existing packaging scripts
do not yet bundle the new executable. A loose build must place `zeus-companion`
beside `zeusd-rs`.

```sh
rtk cargo build -p zeus-engine --bin zeusd-rs -p zeus-companion
rtk ./target/debug/zeus-companion init /absolute/owner/path/companion
rtk ./target/debug/zeus-companion enroll /absolute/owner/path/companion/config.json /absolute/owner/path/companion/enrollment.json interact http://127.0.0.1:19773
```

Use a new state directory; `init` creates it with mode 0700. Its ancestors must
already exist, have trusted ownership and lack unsafe write permissions. Paths
must be absolute and contain no symlink components; on macOS use the canonical
`/private/tmp` instead of `/tmp` for fixtures. Files must be regular, mode 0600,
owned by the current user and have one hard link. Root-owned sticky shared
directories are accepted; an attacker-writable nonsticky ancestor is rejected.
The same Unix account and root are inside the trust boundary.

Path selection is local-administrator authority (CLI arguments and owner-only
configuration), not an HTTP input. Enrollment output and TLS files may live in
other administrator-selected safe directories. Paths reject `.`/`..`, empty
components and NUL before filesystem side effects. Directory resolution walks
from an open root with `openat(O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)` and checks
each opened descriptor's ownership/mode. Leaf reads, locks, temporary creation,
rename, directory sync and cleanup use that verified directory handle; they do
not reopen the original absolute path. Each auth transaction retains the same
directory handle with its nonblocking lock across load/save, even if the path
is renamed or replaced. Cleanup is armed only after exclusive temporary-file
creation succeeds. A directory-sync error after rename means the update may
already be committed, not that it was rolled back. This is not a sandbox
against other code running as the same Unix account.

Descriptor traversal conservatively requires read and search permission on all
ancestors; search-only directory layouts are not supported by this experiment.

The generated `config.json` defaults to `127.0.0.1:19773`, no TLS and no browser
origins. Configure the exact browser origin in `origins` before browser use.
Loopback HTTP is for isolated development. For a private deployment, configure
an exact assigned literal private/overlay address, `allow_private: true`, and
owner-only `tls_certificate` and `tls_key` PEM files. Binding validates the
address through the OS; wildcard, public, DNS, link-local, IPv4-mapped IPv6 and
implicit private listeners are refused. Private binds require TLS. HTTPS
enrollment origins may use DNS for certificate identity, but the listener bind
is always literal. The operator must provide a matching, trusted origin and
certificate. Forwarded headers do not grant identity or alter bind policy.

Start the Engine with `ZEUS_COMPANION_CONFIG` set to that absolute config path.
The Engine launches the sibling executable with structured arguments and a
cleared environment. Its stdin pipe owns sidecar lifetime: EOF on Engine death
closes the listener and upgraded streams. There is no independent daemon,
automatic restart supervisor or public listener. The standalone `serve CONFIG
ENGINE_SOCKET` command likewise requires an open stdin liveness pipe.

Enrollment accepts `read`, `interact` (read plus text/control), or `lifecycle`
(all implemented scopes). It writes `{origin,server_id,code,expires_at_ms}` only
to the explicit owner-only output file. Codes and bearer tokens must not be
placed in argv, stdout, logs or source control. Codes expire after five minutes
and are consumed once. Device bearer tokens expire after 30 days and are hashed
at rest. Keep the enrollment file private and remove it after pairing.

`zeus-companion devices DIR` lists IDs, revocation and expiry without tokens;
`zeus-companion revoke DIR DEVICE` revokes a local enrollment. The HTTP revoke
route is restricted to the authenticated device itself.

## Enforced budgets

| Resource | Bound |
| --- | --- |
| Request body | 16 KiB; pair/control 1 KiB; actions 4 KiB |
| Prompt text | 8 KiB UTF-8; control bytes rejected except newline and tab |
| Encoded response / screen text | 256 KiB / 128 KiB, explicit truncation |
| Screen geometry | Dimensions at most 512, at most 32,768 cells, valid cursor |
| Internal encoded full snapshot | 1 MiB before RLE allocation and geometry checks |
| Page / metadata scan | 64 results / 8192 records or projects |
| TCP connections / upgraded WebSockets | 32 total, including upgrades / 8 |
| HTTP header count / parser buffer | 32 / 32 KiB |
| TLS, HTTP-header, WebSocket first-frame deadline | 5 seconds each |
| Handler / Engine request deadline | 8 seconds / 5 seconds |
| HTTP connection / WebSocket lifetime | 60 seconds / 300 seconds |
| WebSocket incoming message / write buffer | 2 KiB / 8 KiB |
| WebSocket write deadline | 2 seconds |
| Invalidation queue / gateway replay ring | 16 / 128 entries |
| Request rate | 600 per minute globally, 240 per authenticated device |
| Auth store | 64 devices, 8 active enrollments, 64 KiB encoded state |
| Engine consumed lifecycle mutation IDs | 4096, fail closed when full |

These are admission and allocation bounds, not a measured aggregate RSS bound.
Oversized projections fail closed. The Engine's existing 150 ms registry watcher
observes terminal sequence changes only with companion subscribers, coalescing
screens into payload-free invalidations; it constructs no terminal diffs.

The gateway uses axum/hyper for bounded HTTP parsing and WebSocket framing,
rustls for TLS, and `getrandom`, SHA-256, constant-time comparisons and zeroizing
buffers for auth handling. The reference client uses reqwest and
tokio-tungstenite for bounded decoding, TLS and frame validation. These libraries
avoid adding custom HTTP, TLS or WebSocket implementations. Engine is only a
gateway dev-dependency for isolated conformance fixtures.

## Reproduce validation and smoke

From `zeus/`:

```sh
rtk cargo test -p zeus-companion -p zeus-companion-client --offline -- --nocapture
rtk cargo test -p zeus-client --lib companion_cancelled_and_queue_rejected_requests_release_pending_entries --offline
rtk cargo clippy -p zeus-companion -p zeus-companion-client -p zeus-companion-api -p zeus-client -p zeus-engine --all-targets --offline -- -D warnings
rtk cargo build -p zeus-companion --example fixture --offline
```

The tests use isolated state, real Engine IPC and fixture PTYs, never the user's
daemon or SSH host. Coverage includes guarded prompt delivery, lifecycle
confirmation and leases, gateway/Engine reconnect fencing, first-frame auth,
revocation during body decoding and active events, listener/upgrade shutdown,
filesystem rejection, replay overflow, malformed frames, route allowlisting,
client cursor/geometry validation and cancellation cleanup. The delayed-output
test holds a prompt's PTY output behind a FIFO until command invalidations have
settled, then checks `session.output` reaches the external stream with unchanged
session metadata and control state. The executable test verifies stdin EOF
closes both the listener and an upgraded stream.

For another client, create a canonical 0700 temporary directory and run:

```sh
rtk cargo run -p zeus-companion --example fixture --offline -- /private/tmp/OWNER_ONLY_DIR/enrollment.json
```

Keep stdin open. Readiness prints only the enrollment path; the file contains
the loopback origin and pairing material. Discover the first session to use an
isolated echo PTY. EOF or an input byte shuts down and removes the enrollment
file. The #74 adapter smoke owns this lifecycle automatically:

```sh
rtk proxy env COMPANION_RUST_FIXTURE=/absolute/path/to/zeus/target/debug/examples/fixture node prototypes/companion-web/test/rust-smoke.mjs
```

Run that last command from the integrated repository root. #74 commit
`4bd42b9ee50bb5e2434c339cea4c122da344db56` records two successful actual Rust
adapter runs: pairing identity, projections, controller/text DTOs, PTY echo,
events, reconnect without resend, duplicate command rejection, live Archive and
HTTP 401 with a retained revoked bearer. That smoke polls screens for echo;
output-only invalidation is covered by the separate Rust integration test.

Local macOS debug samples before the final regression additions measured 50
empty session-list requests at p50 487 microseconds / p90 731 microseconds and guarded input
to observed screen at 37,024 microseconds. `request_latency_sample_uses_bounded_projection`
and the prompt tests print fresh samples with `--nocapture`. These include a
local fixture Engine and gateway, use no TLS, and are not production performance
gates or a throughput benchmark. No isolated gateway idle CPU, peak RSS,
private-network latency or prolonged slow-client soak claim is made.

## Remaining shipping gates

- Package and configure the opt-in sidecar with product UX and a supported TLS
  enrollment flow; the gateway does not serve the PWA assets itself.
- Publish and test a machine-readable OpenAPI/WebSocket schema alongside the
  current Rust DTO authority before treating v1 as a stable release contract.
- Validate actual private-overlay TLS deployment and sustained resource bounds.
  This ticket's HTTP path uses local Engine/PTY fixtures; #75 separately covers
  local and fake-SSH Holder parity. It does not prove real SSH through HTTPS.
- Resolve durable mutation outcome handling, execution-time revocation across
  IPC, per-resource grants and timed mobile leases as explicit follow-ups.
  Current device scopes are global; a read scope sees curated metadata and
  visible terminal contents for all available sessions.
- Spawn and external scrollback remain unavailable. Native secure storage/pins,
  public exposure, cloud relay and a production mobile application are outside
  this experimental gateway.

Full workspace checks and release packaging remain the integration parent's
validation responsibility; the commands above state this ticket's focused
checks without borrowing unrelated test or benchmark claims.
