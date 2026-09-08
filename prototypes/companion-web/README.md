# Zeus Companion web prototype (#74)

An isolated, command-oriented mobile/PWA client for the curated Companion API.
This directory is development tooling, outside the Rust workspace and shipped
product. It adds no Swift, Engine behavior, Holder transport, or runtime
dependency to Zeus. The Node server is a **synthetic loopback fixture**, never a
gateway. Real actions go through the #73 Rust gateway and authoritative Engine.

## Run the fixture

Requires Node 22 or newer; there are no package dependencies or install steps.

```sh
cd prototypes/companion-web
rtk npm start
```

Open `http://127.0.0.1:4174/fixture` for a fresh two-minute pairing payload, then
open `/` and enter the payload. Compare its origin and server ID, name the
device, and confirm pairing. Open either synthetic session, select **Take
Control**, confirm, and send a prompt. Local and SSH-labelled fixtures follow
the same API. The fixture never connects to SSH, an Engine, or a real agent.
`COMPANION_FIXTURE_PORT` changes its loopback port. Stop with Ctrl-C.

```sh
rtk npm test
```

Tests need permission to bind ephemeral loopback TCP listeners. They use fresh
in-memory enrollment/device/session fixtures and clean up their listeners.
They do not require a real tailnet or credentials. Static assets alone can be
served independently; the fixture routes must not be deployed.

See [VALIDATION.md](VALIDATION.md) for optional dependency-free Chromium tests,
measured latency/network/idle findings, coverage, and device-validation gaps.

## Gateway contract

The authority is `zeus/crates/zeus-companion-api/src/lib.rs` from #73 (initial
DTO foundation commit `76d4c4a`). Browser-facing DTOs are snake_case and remain
independent of internal `zeus-proto`/`remote_pty` structs. The adapter is
`public/client.js`; its public methods expose only the operations below.

| Operation | External route and key fields |
| --- | --- |
| Pair | `POST /v1/pair`: `api_major`, `expected_server_id`, `code`, `device_name`; response `server_id`, `device_id`, `token`, `scopes`, `expires_at_ms` |
| Negotiate | Authenticated `GET /v1/hello`: `server_id`, `api_major`, `api_minor`, `capabilities`, `engine_epoch`, body/response bounds |
| Browse | `GET /v1/projects`, `/v1/sessions`: `{items,next_offset,engine_epoch}`, pages of at most 64; client cap 512 |
| Detail | `GET /v1/sessions/{id}`: `{session,engine_epoch}`; revision is an opaque Engine precondition |
| Screen | `GET /v1/sessions/{id}/screen`: plain text, geometry/cursor, incarnation, screen sequence, control, exited/truncated facts |
| Take control | `POST /v1/sessions/{id}/control/acquire`: `{expected,takeover:true}`; returns `ControlState` |
| Prompt | `POST /v1/sessions/{id}/text`: `{expected,command_seq,text,submit:true}`; returns `ControlState` |
| Lifecycle | `POST /v1/sessions/{id}/actions`: `engine_epoch`, random `mutation_id`, `expected_revision`, `expected_control`, allowlisted `action` |
| Revoke self | `POST /v1/devices/{device_id}/revoke`: `{}` |
| Events | WebSocket `/v1/events`; first frame `{api_major:1,token,cursor}`; events `{cursor:{stream_id,sequence},kind}` |

Required read capabilities: `projects`, `sessions`, `screen`, `events`.
Interaction additionally requires `interact` scope and `control_lease` /
`send_text` capabilities. Rename/lifecycle require `lifecycle` scope and their
advertised capabilities. Unsupported operations stay disabled. The gateway is
the authorization authority; UI gates are extra protection, not authorization.

Read-only full screens do not attach to a Holder, acquire control, or resize a
PTY. Rotation changes CSS layout only. `ControlState` carries
`epoch:{incarnation,generation}`, `owner:{id,label,role}|null`, and `command_seq`.
Prompt sequence is strictly the next value; it is not replayed, including after
timeout. Lifecycle requests carry one fresh mutation ID, the reviewed revision,
and controller epoch. Confirmation captures the session identity; a later
revision/controller change is rejected by the gateway.

Events invalidate client projections. Updates coalesce over 150 ms and fetch
fresh bounded lists/detail/screen. Duplicate events are ignored; gaps and stream
changes require a fresh projection. Reconnect sends the last validated cursor,
refreshes hello/state, and never reacquires control automatically. An uncertain
mutation blocks subsequent mutations until the user reconnects, inspects the
screen, and explicitly acknowledges that review. Nothing queues while offline.

Bounds: 256 KiB JSON responses/WS frames, 16 KiB request bodies, 8 KiB prompt
text measured as UTF-8, 10-second request deadlines, 512×512 maximum dimensions
and 32,768 total cells, bounded strings/lists, safe-integer sequences, and 128
timing samples. Browser WebSocket APIs allocate a frame before JavaScript sees
it; the Rust gateway must enforce its own pre-allocation wire limits.

## Real gateway deployment

Serve **only `public/`** under the same trusted HTTPS origin as `/v1`, using the
reviewed #76 TLS/reverse-proxy configuration. Configure the gateway's exact
browser Origin allowlist. Preserve HTTPS on the private deployed backend per
the #76 decision; HTTP is allowed only for this loopback synthetic fixture.
The PWA does not configure proxies, certificates, Tailscale, or bind addresses.

The pairing payload entered into the form is:

```json
{"origin":"https://zeus.example","server_id":"EXPECTED_ID","code":"ONE_TIME_CODE","expires_at_ms":1790000000000}
```

Use the actual short-lived payload from the local enrollment tool, not this
example. It is never put in a URL. Browser TLS validates the origin; compare the
ID out of band on the trusted computer. `expected_server_id` binds the exchange
before code consumption, then PairResponse and Hello must match. Browsers cannot
independently pin a raw certificate/key fingerprint. This prototype intentionally
uses same-origin serving; its CSP rejects arbitrary cross-origin gateway URLs.

## Privacy, credentials, and accessibility

- Tokens exist only in private JavaScript memory. No localStorage,
  sessionStorage, IndexedDB, cookies, credential URLs, clipboard writes, logs,
  analytics, notifications, or crash-reporting integration. Reload/close requires
  a new pairing. JavaScript cannot guarantee memory zeroization or Keychain-level
  protection against compromised browser extensions/processes.
- Requests use `Authorization: Bearer`, omit browser cookies, refuse redirects,
  and request `no-store`. TLS endpoints/proxies must suppress credential/body
  logging and set `Cache-Control: no-store` on API responses. The service worker
  caches only an exact public shell asset allowlist, never API data or requests
  carrying Authorization. Unpair deletes local data and the shell cache, tries
  server revocation, and clearly reports when revocation remains unconfirmed.
- A manual **Hide screen** control and `visibilitychange`/`pagehide` curtain
  remove sensitive DOM content and disconnect background work. Returning requires
  explicit **Reveal & reconnect**. Pending prompts/confirmations are discarded.
  **A PWA cannot guarantee iOS app-switcher snapshot timing or prevent OS/browser
  captures. Hide before switching apps; this is a production gate, not a claim
  of native privacy parity.** Printing also hides all session content.
- Terminal and metadata are inserted using text nodes only, never HTML/ANSI
  interpretation. Control and bidi override characters are replaced. No raw
  keys, mouse/focus events, resize, signals, generic RPC, or arbitrary execution.
- Responsive rem typography, browser zoom, safe-area insets, labelled forms,
  visible keyboard focus, skip link, native confirmation dialog with Cancel as
  default focus, polite status announcements, 48px action targets, scrollable
  named terminal regions, and reduced-motion rules. Terminal output is not a
  live region, so VoiceOver is not flooded by each update.

## Findings and production decision

**Go for API/workflow validation; no-go for production/mobile privacy claims.**
The deterministic fixture proves pairing, local/SSH-unified browsing, explicit
control, prompt/status/screen round trips, lifecycle confirmation, revocation,
bounded decoding, and reconnect without replay. It does not prove actual SSH
routing, Engine race guarantees, real TLS/overlay behavior, or native iOS privacy.
Use the #73/#75 integration suites for Engine authority and controller fencing.

Bounded recent output and spawn are unavailable in the current gateway contract;
the UI says so for recent output instead of inventing an endpoint. Full terminal
parity, push/APNs, background delivery, cloud relays, persistent browser tokens,
and arbitrary desktop controls are outside this prototype.

Production backlog: real Rust gateway smoke, private HTTPS/overlay soak,
real-device Safari/PWA portrait/landscape and large-text/VoiceOver checks,
controller handoff with an active desktop, revocation during suspension,
app-switcher capture audit/native client decision, battery/energy measurements,
network/latency profiling, secure device rotation/recovery, and bounded recent
output when the gateway advertises it. SVG install icons may need platform-sized
PNG assets for native home-screen polish. No code here changes the completed
Remote Holder architecture or its release gates.
