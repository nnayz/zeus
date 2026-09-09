# Companion architecture and trust model

## Decision, evidence, and readiness

This decision resolves [#76](https://github.com/nnayz/zeus/issues/76) under
[#72](https://github.com/nnayz/zeus/issues/72). **Go for the bounded experimental
spike; no-go for production.** The selected architecture is an Engine-launched
narrow Rust sidecar using `zeus-client`, with an independent external v1 API:
REST/JSON over HTTPS and WSS. The authoritative local Rust Engine owns sessions,
status, orchestration, host state, lifecycle policy and mutation/control fencing.

The experimental contract is grounded in #73's `zeus-companion-api` foundation
commit `76d4c4a`, #75's terminal seam `137fda6`, and the gateway worktree reviewed
on 2026-09-07: `zeus/crates/zeus-companion/src/{main,server,config,auth}.rs` and
`zeus/crates/zeus-engine/src/control/companion.rs`. That gateway now mounts the
terminal routes and dispatches the lifecycle actions described below. Gateway
validation remains separate from this documentation change. A DTO or advertised
capability is not
proof of a working operation. This document distinguishes the concrete spike
from unimplemented production requirements, rather than defining a second
endpoint contract for #73/#74/#75 clients.

V1 is command-oriented: read-only full screen snapshots and selected commands
with explicit controller takeover. Raw mobile resize, signal, PTY bytes and
terminal diffs are excluded. The PWA prototype lives at
`prototypes/companion-web` in this repository. Rust gateway/API/reference-client
code lives under `zeus/`. Native iOS lives in a separate repository, consuming
external schemas and fixtures; no Swift enters this repository or runtime.

## Authority and data flow

```text
PWA / separate-repository iOS / typed Rust HTTP reference client
  |  HTTPS REST/JSON + WSS, external API v1, per-device authentication
  |  explicitly selected private network; overlay is reachability only
  v
Engine-launched Rust Companion sidecar
  |  TLS, device authentication, curated DTOs, bounded event delivery
  |  zeus-client -> local IPC -> verified Rust Engine Hello
  v
Authoritative local Rust Engine
  |  SessionRecord, status, project/host state, orchestration, lifecycle
  |  metadata mutation epoch/revision fence; terminal controller/command fence
  +--> local Holder -> Agent
  +--> ssh -T -> Bridge -> Remote Holder -> Agent
```

The sidecar never owns a shadow session registry, status reducer, scheduler,
PTY, SSH process, Agent lifecycle or multi-session remote supervisor. The
spike's sidecar `AuthStore` owns device credential metadata, separate from
Engine session state. Gateway-authenticated device IDs accompany curated
Engine calls. Production must verify a narrow Companion IPC role and
execution-time device authorization; same-user IPC is not an OS sandbox.
Declaring `ClientRole::Mobile` alone is not an authorization boundary.

`remote_pty` remains the internal versioned Engine-to-Helper protocol. Phones
never receive Holder tokens, access the Engine Unix socket, attach to a Holder,
or invoke SSH. `zeus-remote` gains no mobile behavior. `zeus-node` keeps its
separate enhanced-node/account/usage/handoff boundary and is not the gateway
or a bootstrap dependency. The [security model](security-model.md) still applies.

## Lifecycle decision and verification gates

Companion is opt-in and launched by the Engine as a packaged Rust sidecar after
configuration and identity checks. No service installation, elevation, overlay
configuration or independent startup supervisor is approved. The following is
the selected lifecycle to implement and test; the inspected gateway alone does
not establish every row.

| Trigger | Selected behavior and verification needed |
| --- | --- |
| Enable | Start one matching sidecar; default loopback. Validate config/TLS before opening the configured listener. |
| Disable or explicit Engine shutdown | Stop admission and close connections; Engine policy resolves accepted work. Never let the gateway kill/adopt Agents. |
| Sidecar crash | Session owners survive. Report unavailable; local explicit retry is sufficient for the spike. Automatic restart policy is deferred. |
| Engine loss | Reject actions with `engine_unavailable`; fence old requests after reconnect. IPC disconnection emits an invalidation event; the sidecar's Engine-owned stdin pipe triggers shutdown on EOF. Production must prove both crash cleanup and transient IPC reconnect admission. |
| Upgrade | Replace the sidecar through Engine lifecycle, renegotiate API/Engine epoch and invalidate event cursors. Do not transfer session ownership into the sidecar. |
| Desktop app quit | Stop Companion. It must not retain an otherwise idle Engine; live Holders follow existing Engine survival policy. Unattended mobile access is deferred. |
| No active sessions | With the desktop open and Companion enabled, bounded discovery may remain available. A paired phone alone does not keep a service alive after app quit. |

The sidecar waits up to 10 seconds for Engine connection and verifies Companion
Hello before binding. Its Engine-owned stdin pipe drives server shutdown on EOF;
the server aborts its tasks on shutdown. It has no durable outcome journal. Startup,
app-quit, crash, upgrade and controller cleanup need integration evidence before
release; a bounded HTTP timeout does not prove an Engine mutation was cancelled.

## Experimental external protocol

External v1 is independent of Engine RPC and `remote_pty`. The foundation uses
`API_MAJOR = 1`, `API_MINOR = 0` and curated serde DTOs in
`zeus-companion-api`. **OpenAPI 3.1 is the selected production wire and
client-generation authority**, including JSON Schema components for WSS
payloads and documented handshake/order/close behavior. At inspection the
foundation had Rust DTOs but no authoritative OpenAPI artifact. #73 must publish
it and prove schema/implementation/client conformance. This ADR does not
substitute invented schema or endpoint names for that deliverable.

`Hello` contains `server_id`, `api_major`, `api_minor`, `capabilities`,
`engine_epoch`, `max_body_bytes`, and `max_response_bytes`.
Current capability strings are `projects`, `sessions`, `screen`, `events`,
`rename`, `lifecycle`, `control_lease`, and `send_text`.
Clients fail closed on unknown major, wrong server
identity or missing capabilities they require. The spike checks major in
pairing and WSS subscription; it does not implement a separate required-capability
request header. Static capability lists can include operations awaiting
integration. Truthful per-operation advertisement and a cross-version matrix
are production gates, not guarantees established by those constants.

`Project` exposes ID, name, root and optional host label. `Session` exposes ID,
project ID, kind, title, cwd, host label, reduced status, creation/update times,
archive/hibernate flags and Engine-issued `revision`. `SessionDetail` adds
`engine_epoch`; `Page<T>` contains `items`, `next_offset` and `engine_epoch`.
Pagination uses offset/limit, not an opaque cursor. No environments, provider
credentials, SSH configuration or raw `SessionRecord` serialization is exposed.
Current scopes apply to the available projection as a whole; per-project/session
resource grants are unimplemented follow-up work.

### Route and operation allowlist

These are the concrete routes in the reviewed gateway. The table does not
certify their production readiness.

| Route | Authentication/scope | Experimental behavior |
| --- | --- | --- |
| `GET /v1/hello` | Bearer, `read` | External `Hello`; gateway supplies persisted server identity. |
| `POST /v1/pair` | Unexpired enrollment code | `PairRequest` -> `PairResponse`; reject wrong major/server or invalid/consumed code. |
| `GET /v1/projects` | Bearer, `read` | Bounded `Page<Project>` using `offset` and `limit`. |
| `GET /v1/sessions` | Bearer, `read` | Bounded `Page<Session>`. |
| `GET /v1/sessions/{id}` | Bearer, `read` | `SessionDetail`. |
| `GET /v1/sessions/{id}/screen` | Bearer, `read` | Fixed full-snapshot request through Engine, projected as bounded plain-text `Screen`. |
| `POST /v1/sessions/{id}/control/acquire` | Bearer, `interact` | `AcquireControl` -> `ControlState`; explicit takeover. |
| `POST /v1/sessions/{id}/control/release` | Bearer, `interact` | `ReleaseControl` -> `ControlState`; expected controller epoch. |
| `POST /v1/sessions/{id}/text` | Bearer, `interact` | `SendText` -> `ControlState`; expected epoch and command sequence. |
| `POST /v1/sessions/{id}/actions` | Bearer, `lifecycle` | `Mutation` enum rename/archive/wake/hibernate/terminate. Live lifecycle requires `expected_control`; unsupported actions fail closed. |
| `POST /v1/devices/{id}/revoke` | Bearer, `read`, own device only | Authenticated ID must equal route ID. Other device administration is local. |
| `GET /v1/events` | First WSS frame bearer, `read` | Bounded invalidation events, no data before authentication. |

The reviewed Engine dispatches rename, archive, wake, hibernate and terminate.
Non-rename actions require `expected_control` for a live session and
`Session::validate_terminal_control` under the registry lock, along with revision,
Engine epoch and consumed mutation ID checks. Archive terminates before archiving;
archive/terminate use a 3-second termination wait. Validation must cover these
effects and their failure states before production. No alternate `/capabilities`,
`/pairings/redeem`, `/snapshot`, stream-ticket, mutation-status or mutation-fence
endpoints are part of this experimental contract.

Scope vocabulary is `read`, `interact`, `spawn`, `lifecycle`; an unused scope
does not enable an operation. Spawn, structured question answers, independent
host listing, recent-output history and remote enrollment management have no
implemented route in the inspected spike. Additions require schema, Engine
allowlist and conformance review. Scope grants do not imply other scopes.

Everything outside this allowlist is denied, particularly:

- generic Engine RPC/method dispatch, socket export, direct Holder attach,
  Helper tokens, `remote_pty` or `zeus-node` passthrough;
- raw PTY input/keycodes/escape sequences, mobile resize/signal/scroll/diffs,
  mouse events, arbitrary process signals or generic kill;
- arbitrary shell/command/argv/cwd/environment overrides, arbitrary files,
  provider keys, authentication responses and full environment access;
- host edits, SSH/helper installation/GC, network/tunnel/port configuration,
  MCP forwarding, hooks, browser automation, updater/daemon administration;
- Git/worktree mutations, force operations, checkpoint/handoff/account APIs,
  scope escalation, public sharing, cloud relays, APNs/background push.

Text submission may make an Agent run powerful tools under the desktop account.
`interact` is substantial execution authority even without an arbitrary-command
endpoint. Confirmation flags express user intent; they are not a boundary
against a compromised authorized client. Engine eligibility, scopes and
controller checks remain necessary.

### Full screen and explicit controller contract

`Screen` carries `session_id`, `incarnation`, `screen_sequence`, plain `text`,
`cols`, `rows`, `cursor_row`, `cursor_col`, `control`, `exited`, and `truncated`.
It is a full bounded text screen projection, not raw ANSI, a grid delta, full
scrollback or native rendering parity. Render text inertly. Display `truncated`
explicitly; never call a truncated projection a complete terminal capture.
Phone layout scrolls/scales existing geometry and sends no PTY resize. Refresh
screen after invalidation; WSS carries no terminal snapshot bytes in this spike.

Read-only screen access must use Engine state without stealing a Holder attach.
Unavailable/stale state must be surfaced honestly. The fixed full-snapshot
request uses protocol 1 and `since: None` through Engine and must preserve the
existing single Holder attach; multiple Helper
observers remain deferred. Local and SSH-backed sessions use the same curated
external API through Engine.

`ControlEpoch` is `{incarnation, generation}`. `ControlState` adds optional
`owner` (`id`, `label`, `role`) and `command_seq`. `AcquireControl` sends
`{expected, takeover}`; takeover must be an explicit confirmed user choice.
`ReleaseControl` sends `{expected}`. `SendText` sends
`{expected, command_seq, text, submit}`. Engine arbitrates desktop/mobile
writers, rejects stale incarnation/generation and repeated/out-of-order command
sequence, and exposes the resulting owner. It must not replay text after
uncertain delivery or silently reacquire control on reconnect. Disconnect and
revocation cleanup are integration gates; a suggested 60-second expiring lease
is not present in these DTOs and is not an implemented guarantee.

The gateway rejects text over 8 KiB and control characters except newline/tab;
it never translates client-supplied escape/keycode streams. It reauthenticates
after decoding mutation/control/text bodies and before forwarding, and checks
read authorization again before returning a screen. Execution-time revocation
inside the Engine still requires separate proof.

### Mutation and event semantics

Metadata `Mutation` contains `engine_epoch`, a 16..64 byte ASCII `mutation_id`,
`expected_revision`, optional `expected_control`, and tagged `action`. The
Engine hashes its session projection for revision, serializes mutation checks,
and keeps up to 4,096 consumed `(device_id, mutation_id)` request digests for its
current random epoch. Same key/payload is rejected as `replayed_mutation`;
changed payload is `mutation_conflict`. It does not return cached success.
Old Engine epoch or stale revision fails before dispatch. The consumed-key map
never evicts to admit more work: saturation fails closed until a new Engine
epoch. Success is `{mutation_id, applied}`; lifecycle enums carry `confirmed`
and require safe controller/lifecycle integration.

This is Engine-owned bounded replay rejection, not durable exactly-once
execution. Restart loses consumed records but changes epoch, rejecting old
requests. Timeout or crash after an effect may leave outcome unknown. Clients
refresh state and seek a new explicit action; never replace ID/epoch and retry
automatically. A durable outcome journal, short-lived execution tickets/fences
and mutation-status lookup are proposed hardening, not current DTOs/endpoints.
Terminal command sequencing is a separate Engine fence, not metadata replay.

The first WSS text frame is `Subscribe {api_major, token, cursor}`; `token` is
the device bearer, not a one-time ticket. It must arrive within 5 seconds and
never goes in a URL. `Cursor` is `{stream_id, sequence}`. Server `Event` contains
that cursor and `kind`: `changed`, `resync_required`, or `engine_unavailable`.
These invalidate projections so clients fetch fresh bounded REST state. WSS
accepts no commands.

The gateway retains 128 events globally. A cursor in the same stream/window
replays later invalidations; absent/expired/future/wrong-stream cursors receive
`resync_required`. Stream identity changes on restart. Subscription and replay
are captured under the event-hub lock; clients deduplicate cursors. A lagging
receiver gets resync rather than a growing queue. A blocked write ends the
stream after 2 seconds. Reconnect reauthenticates and refetches state, never
replays mutations. There is no time-based/per-device terminal replay store.

Errors use `ApiError {code}`. Examples: 401 `unauthorized`/`pairing_denied`;
403 `origin_denied`/`own_device_only`; 404 `not_found`;
400 `invalid_request`/`confirmation_required`; 413 `body_limit`;
415 `json_required`; 426 `version_mismatch`; 409 `stale_engine`,
`stale_revision`, `replayed_mutation`, `mutation_conflict`, `engine_rejected`;
429 `busy`/`rate_limited`/`connection_limit`; 501 `capability_unavailable`;
502 `projection_limit`; 503 `engine_unavailable`; 504 `outcome_unknown`.
Terminal mappings include 409 `stale_controller_epoch`, `not_controller`,
`controller_busy`, `command_sequence`, `input_unconfirmed`; 422
`terminal_geometry`; and 502 `invalid_snapshot`/`snapshot_required`.
Unlisted Engine errors collapse to `engine_rejected`. Engine-side
`outcome_unknown`, `input_unconfirmed`, and `terminal_unavailable` retain their
sanitized codes (409); client timeout maps to 504 `outcome_unknown`. Regression
tests cover these mappings. Clients must treat uncertain delivery as requiring
fresh state and explicit review, never an automatic mutation retry. Sanitized
close behavior remains a production review item.

### Observed bounds and remaining budget gaps

These values describe the inspected spike, not an invented second set of limits.
An undemonstrated queue/allocation/operation budget blocks production rather
than being described as implemented protection.

| Resource | Concrete spike bound |
| --- | --- |
| DTO limits | `MAX_BODY` 16 KiB, `MAX_RESPONSE` 256 KiB, `MAX_TEXT` 8 KiB, `MAX_PAGE` 64, projected strings 512 bytes. IDs at most 64 ASCII alphanumeric/underscore/hyphen bytes. |
| Specific HTTP bodies | Pair/acquire/release 1 KiB; metadata action 4 KiB; self-revoke 16 bytes; text 16 KiB body with 8 KiB text. JSON content type required. |
| Lists | Limit clamped to 1..64; offset and registry/project count at most 8,192, otherwise projection error. |
| Screen | Text truncated at 128 KiB on a UTF-8 boundary; encoded response at most 256 KiB, with geometry/ownership and `truncated`. Pre-serialization and terminal-seam allocation bounds need integration evidence. |
| HTTP transport | HTTP/1 only; 32 connections, 32 headers, 32 KiB parser buffer, 5-second header/TLS handshake deadlines, 60-second connection lifetime. |
| HTTP operations | 8-second gateway request deadline including Engine calls. Expiry returns unknown outcome, not proof of Engine cancellation. |
| WSS | 8 connections; client frame/reassembled message 2 KiB; write buffer 8 KiB; first auth frame 5 seconds; send deadline 2 seconds; stream lifetime 300 seconds. |
| Event buffers | 128 retained invalidations; broadcast channel 16 events. Bounded kind/sequence/random stream ID, no terminal payload; lag triggers resync. |
| Request rates | 600 requests/minute globally, 240 authenticated requests/minute/device; fixed-window device map at most 64 entries. WSS admission authenticates and counts the device. |
| Auth state | 64 devices, 8 pending enrollments, 64 KiB state file; nonblocking file lock fails if busy. Device name 1..64 bytes, no controls. |
| Auth time | Enrollment 5 minutes; bearer 30 days. WSS reauthenticates before each replay/live event and on a 1-second idle timer; check-to-write and queued-effect races remain test gates. |
| Metadata replay | 4,096 consumed device/ID digests per Engine epoch, no eviction; try-lock returns busy. Bounded 64-byte IDs and fixed digests. |
| Config/files/startup | At most 8 exact Origins, 256 bytes each; config read 16 KiB, certificate read 64 KiB, private key read 16 KiB; owner-only files; Engine connect wait 10 seconds. |

Production must account for every queue/allocation across HTTP, WSS,
`zeus-client`/IPC and Engine: outstanding operations, cancellation time, screen
projection before serialization, JSON depth, expanded payloads, total memory
and CPU. A limit checked after serialization does not establish bounded
intermediate allocation. Shared Engine queues/deadlines, terminal bounds and
revocation during replay/queued effects need measurements and tests. No
unbounded fallback or per-client map may close a feature gap.

## Literal binding, TLS, and browser boundary

The spike defaults to `127.0.0.1:19773`. `Config.bind` is a literal `SocketAddr`;
it does not resolve hostnames/MagicDNS. `allow_private` must be explicitly set
for non-loopback; the OS must be able to bind the selected address.

| Address class | Current spike policy |
| --- | --- |
| IPv4/IPv6 loopback | Allowed without private opt-in. |
| RFC1918 IPv4 | Exact literal in `10/8`, `172.16/12`, or `192.168/16`, explicit private opt-in. |
| Tailscale CGNAT | Exact literal `100.64/10`, explicit private opt-in. |
| IPv6 ULA | Exact literal `fc00::/7`, explicit private opt-in. |
| Hostnames, IPv6 link-local, IPv4-mapped IPv6 | Conservatively rejected/unsupported; no DNS resolution or scope-ID acceptance algorithm. |
| Wildcard/public/other | Rejected, including `0.0.0.0`, `::`, public unicast and multicast; no public override. |

DNS/MagicDNS or scoped link-local support is optional future work, not needed
for the literal-address spike. Later hostname support must validate every
resolved answer, reject mixed public/private sets, and bind a selected validated
address without rebinding widening exposure. Test conservative rejections and
interface loss. Zeus never enables Tailscale, Headscale, WireGuard, routes,
Funnel, NAT forwarding or public exposure; overlay membership grants no scope.

Private binds require both TLS certificate and key. Rustls supplies TLS; clients
must validate trust, hostname/SAN and expiry without ATS exceptions.
Vendor-neutral operator-provided certificates (public CA for an owned name or
an explicitly device-trusted private CA) and operator-obtained Tailscale
certificates are viable sources. Zeus installs no CA and does not obtain or
configure overlay certificates. Proxy identity headers never authorize a device.

The experimental config permits HTTP on loopback without TLS files and explicit
loopback HTTP Origins for fixtures. This is a local test/backend facility, not
approval for plaintext mobile access. The PWA uses a same-origin HTTPS reverse
proxy. Tailscale Serve may be evaluated as an operator-managed private TLS
terminator (never Funnel); upstream transport/provenance, Host/SNI, certificate
renewal and real iOS/browser validation remain deployment gates. No mobile
client may use HTTP or `ws://`; no TLS verification or ATS exception is approved
to make deployment work. Direct gateway TLS is the preferred vendor-neutral
private listener.

The inspected CORS policy allows at most eight exact Origins and
Authorization/Content-Type headers, without ambient cookie authorization.
Origin absence is possible for native clients and is not proof of trust;
authentication is mandatory independently of Origin. Strict Host/SNI handling,
null/duplicate Origin tests, request-smuggling/redirect/proxy tests and safe
first-frame auth failure behavior remain production review items.

## Pairing, identity, scopes, and recovery

Local enrollment chooses scopes and generates a random 256-bit code (64 hex
characters), expiring after 5 minutes. QR/manual payload:
`{origin, server_id, code, expires_at_ms}`. `PairRequest` requires
`{api_major, expected_server_id, code, device_name}`. Wrong identity is rejected
before consuming the code; successful redemption is atomically single-use.
`PairResponse` is `{server_id, device_id, token, scopes, expires_at_ms}`.
Clients compare both `PairResponse.server_id` and authenticated `Hello.server_id`
to the locally approved identity. Browser PKI and accurate origin transfer are
essential: an application server ID is not a TLS certificate pin.

Each device gets a random 256-bit bearer for 30 days. The inspected `AuthStore`
stores SHA-256 digests of high-entropy codes/tokens with constant-time comparison;
it stores neither plaintext nor a per-record salt. Owner-only `0700` directories
and `0600` files, symlink/type/owner/link-count checks, atomic writes and a
nonblocking file lock protect state. Hashes do not protect a stolen bearer or
compromised desktop account.

Local auth metadata lists device IDs/names/scopes/expiry/revoked state; the CLI
lists IDs/expiry/revoked state and revokes devices.
external self-revocation is allowlisted. Scope expansion is not remote. Rotation
and recovery are revoke then freshly enroll, not a `/rotate` endpoint. Lost
pairing response, phone replacement, expiry or restored backup requires new
local enrollment. Revoke a stolen phone locally. Per-resource grants,
credential-generation fencing, atomic rotation and a second desktop confirmation
during redemption are unimplemented hardening options, not current guarantees.

Bearers support Rust/PWA/native clients simply, but copying a token copies its
scopes until expiry/revocation. Asymmetric keys with signed requests could
resist copied bearers but need replay/signature review. mTLS adds certificate
management and browser interoperability costs. Native SPKI pinning could
supplement normal TLS with local re-pairing on identity change; the PWA cannot
inspect SPKI through ordinary browser APIs. None of these alternatives is
implemented or grounds for bypassing TLS. One-time WSS tickets are a future
option, not the current first frame.

Revocation makes subsequent authentication fail; WSS checks before each replay
and live event and at idle intervals. It is not an instantaneous guarantee
across the check-to-write interval, queued Engine work or controller leases.
Production must test a single execution/delivery boundary for revoked authority;
delivered data and committed effects cannot be recalled. Auth unavailability
must fail closed.

Native credentials belong in unlocked-access, non-synchronizing, device-only
Keychain storage in the separate iOS repository. PWA tokens stay in memory;
reload requires re-pairing. No localStorage/IndexedDB/token cookie, URL credential
or service-worker token cache. Cache static shell only; API responses are
`no-store`. No analytics, notifications, prompt/terminal logs or third-party
scripts are approved. Hide content on visibility/pagehide and manual Hide;
this cannot guarantee iOS app-switcher captures exclude it. Native/real-device
privacy validation remains a production gate.

Logs, diagnostics and crash reports must exclude codes, tokens, authentication
responses, prompts, full environments and terminal payloads. Terminal text may
contain source, paths and secrets even for `read` devices. Server state/backups
are sensitive; restore must not silently reactivate revoked credentials.
Metadata-only diagnostics, CSP/referrer protection, backup exclusion/identity
reset and redaction fault tests are release requirements.

## Threat model

| Threat | Current control or selected requirement | Residual risk / production evidence |
| --- | --- | --- |
| Stolen/restored phone | Unique expiring bearer, local revocation; PWA memory only; native device-only Keychain decision. | Compromised client can use its scopes; test restore/re-pair, real-device privacy and active revocation. |
| Leaked enrollment/bearer | High entropy, 5-minute one-use code, hashed state, rate limits, scoped token. | Leaked code can be redeemed first; no second approval/pinning claim. Test parallel redemption and lost responses. |
| Hostile LAN/tailnet peer | Private literal bind, private TLS, independent application auth. | Membership grants nothing; prove unauthenticated/incorrect-scope rejection. |
| DNS/MagicDNS confusion/wrong server | No bind DNS; expected server ID in pairing/responses; validated client TLS. | Server ID is not a pin; test malicious origins, SAN mismatch and any future DNS support. |
| Duplicate/replayed mutation | Engine epoch/revision/consumed-ID fence; terminal control/command sequence. | No durable result cache/exactly-once claim; crash can be unknown. Test retries/restarts without redispatch. |
| Downgrade/capability confusion | Major in DTO handshake, curated API. | Static capabilities can overstate readiness; prove truthful operation negotiation. |
| Cross-origin/WS hijack | Exact Origins, bearer REST and first-frame WSS auth, no cookie authority. | Test absent/null/duplicate Origin, CSRF/CORS, unauthenticated upgraded sockets; same-origin XSS remains serious. |
| Oversized/slow/exhausting clients | Wire/socket/rate/time bounds, bounded ring and lag resync. | Prove IPC, JSON/screen intermediate allocation and Engine deadline budgets. |
| Stale/revoked subscriptions | Bounded cursor replay, stream IDs, replay/live auth checks. | Check-to-write and queued-effect races need tests; no immediate cancellation guarantee. |
| Malicious terminal/Agent output | Inert full text; no ANSI/HTML or automatic links/clipboard/actions. | Test OSC/escape/script/bidi spoofing; output never authorizes user actions. |
| Accidental public listener/proxy | Literal whitelist, no wildcard/public override or network setup. | Check proxy/Host/SNI, interface policy and no public forwarding. |
| Logs/crashes/backups | Protected auth files, redacted secret DTOs, no payload logging/cache design. | Canary scan logs/panic reports/backups and restored state; hashed state is still sensitive. |
| Confused deputy/gateway compromise | Curated operations, Engine-owned sessions/fencing, no generic RPC/Holder API. | Narrow IPC/device execution authorization needs proof; same-user code is outside sandbox claim. |
| Desktop/mobile control race | Expected incarnation/generation/sequence; explicit takeover. | Prove shared desktop arbitration, disconnect/revoke/terminate races and no read-triggered takeover. |

## Deterministic tests and measurements

Use fixture homes/clocks, loopback TLS with fixture CA, literal-address tables,
fake Engine/`zeus-client` peers, Unix sockets, spawned PTYs and fake SSH. No
real tailnet, personal remote host, production credential, privileged bind,
installed service or provider account is needed.

| Suite | Required evidence |
| --- | --- |
| Contract (#73) | OpenAPI/codegen fixtures for actual `/hello`, `/pair`, `/screen`, `/actions`, control/text, first-frame bearer/events; wrong major/server, unknown fields and all denied operations fail. |
| Auth/TLS/bind (#73) | Literal ranges and conservative rejections, no wildcard/public, trust/expiry/SAN/Origin failures, pairing replay, expiry/revoke/re-enroll, file/symlink safety. |
| Engine metadata (#73) | Same/changed duplicate IDs, stale revision/epoch, 4,096-record saturation, persist failures/restart; no automatic retry after unknown outcome. |
| Terminal/controller (#75) | Bounded full text/truncation, fixed geometry, no raw resize/signal/diff, desktop/mobile takeover and command sequence, disconnect/revoke/lifecycle integration. |
| Replay/slow clients (#73/#75) | Window replay, expired/wrong cursor, ring/channel overflow, Engine outage, sidecar restart, blocked writes; resync/refetch without duplicate commands. |
| Lifecycle (#73) | Enable/disable, app quit, no sessions, crashes/upgrades; surviving Holder PID/incarnation and no sidecar Agent ownership. |
| Client (#74) | PWA pairs/lists/inspects/takes control/sends text only when supported, observes fresh screen; separate native TLS/Keychain/real-device privacy checks. |
| Abuse/privacy | Oversized/slow fragmented JSON/WSS, exhaustion, terminal injection, revoke during replay/queued work, canaries absent from logs/caches/reports. |

Publish release latency distributions, RSS/CPU, network bytes and idle wakeups
with hardware/sample sizes, with/without Companion under equal load. Include a
23 MiB output burst and 30-minute slow/disconnected-client soak to prove memory
plateaus and PTY/Engine progress. Account for all queue/frame/replay/operation
bounds before selecting final budgets; constants are not measurements. No
new fan-out, lock or cache is justified without evidence. Preserve current
`zeus/REMOTE_PORT.md` Helper gates: snapshot p90 <= 100 ms, input-to-PTY p95
<= 10 ms, output-to-diff p90 <= 8 ms, loopback interaction p50 <= 75 ms / p90
<= 150 ms. Private-network RTT is reported separately and never excuses
correctness or session-identity failures.

## Go/no-go gates and unimplemented follow-up

**Go:** #73 continues the Rust gateway/reference-client/OpenAPI spike; #75
integrates full screen and shared controller fencing; #74 uses the in-repo PWA
and keeps native iOS separate. Keep clients on the actual experimental routes
and DTOs. Missing operations return unavailable, never generic RPC.

**No-go for production** until recorded evidence establishes:

1. Authoritative OpenAPI/WSS schemas match router, generated clients, Engine
   operations and truthful capabilities, including unsupported cases.
2. HTTPS/WSS works on real clients without ATS exceptions; literal bind,
   deployment/proxy/Origin identity and private-only access are verified.
3. Revocation/scopes fence queued effects/deliveries; credential recovery,
   backups, rotation and narrow IPC authority are reviewed.
4. Engine metadata/terminal fences survive duplicate/timeout/crash scenarios
   with honest unknown outcomes. If durable result recovery is needed, review
   and implement an Engine journal; the in-memory spike does not have one.
5. Every queue/allocation/replay window/Engine operation has a documented,
   enforced budget; abuse/performance tests pass without blocked PTY drain
   or new idle Holder work.
6. App-quit/crash/upgrade/no-session behavior and desktop/mobile cleanup preserve
   session identity; real-device privacy and repository boundaries are verified.

Potential hardening includes durable Engine outcomes and short-lived mutation
fences, one-time WSS tickets, per-resource grants, atomic credential rotation,
native pinning/device keys and expiring controller leases (60 seconds was
suggested). These are unimplemented follow-up decisions, not current fields,
routes or guarantees. Select/specify their limits before enabling them; they
must not weaken existing Engine fencing or TLS. Hostname/link-local support is
an optional evaluated extension. Public/wildcard exposure, relay, full raw mobile
terminal and unattended service require a new reviewed decision.

## Separate pre-existing baseline mismatch

`zeus/REMOTE_PORT.md` says legacy SSH/tmux retirement paths were removed, but
this checkout retains `zeus-engine/src/legacy_remote.rs`, its `src/lib.rs`
export, `src/control.rs` invocation and `zeus-engine/tests/legacy_remote.rs`
(under `zeus/crates/`). This existing implementation/documentation mismatch is
not a Companion dependency or an approved fallback. For #76 preserve the Remote
Holder-only design, record the mismatch separately, and leave that code
unchanged as directed. Reconcile retirement in a separate maintenance change;
do not silently normalize it in the Companion contract.
