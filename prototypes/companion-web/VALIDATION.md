# Prototype validation record

The Node conformance fixture and Chromium UI exercise the #73 external DTOs.
These are prototype findings, not production performance gates or a substitute
for the Rust Engine/gateway integration suites.

## Commands

From `prototypes/companion-web`, with Node 22 or newer:

```sh
rtk npm test
rtk npm run measure
```

The browser tests are opt-in and skipped when `COMPANION_BROWSER` is absent.
Point it at an existing Chromium/Chrome executable; the harness uses a fresh
temporary profile and a private CDP pipe, then cleans both up. It does not install
packages, reuse the user's browser, or expose a debugging TCP port.

```sh
rtk proxy env COMPANION_BROWSER=/absolute/path/to/chromium \
  node --test test/client.test.mjs test/browser.test.mjs
```

Set `COMPANION_SCREENSHOT_DIR` to save synthetic pairing/session/privacy PNGs.
The Zeus browser MCP was unavailable in this environment (`test sidecar not
found`), so the recorded browser checks used installed HeadlessChrome
151.0.7922.34 directly. This is Chromium mobile emulation, **not Safari/iOS**.

## Coverage

The 20 client/fixture tests cover same-origin TLS restrictions, short-lived
pairing and expected identity, single-use codes, local/SSH-labelled browsing,
read-only viewing, explicit controller acquisition, prompt delivery, lifecycle
scope/confirmation/revision fencing, command sequencing, stale desktop ownership,
ambiguous delivery, revocation, HTTP/WS size limits, malformed JSON/UTF-8,
geometry, unknown required capabilities, event duplicate/gap/resync cursors,
deadlines, and forbidden origins/paths. Live Archive warns that it terminates
the agent and may lose unfinished work; its regression requires explicit
confirmation/current mobile control and verifies an exited archived session.

Three browser workflows cover:

- Pairing sends no credential/code until explicit identity confirmation; inspect
  an SSH-labelled session, Take Control, send a prompt, observe screen/status,
  cancel termination, confirm rename, hide/reveal, desktop handoff, and revocation.
- 320×568 and 375×812 portrait, 812×375 landscape, 390×844 reference viewport,
  200% root font size, no page-wide horizontal overflow, at least 44px controls
  (design target 48px), labelled fields, Cancel dialog focus, reduced motion,
  inert malicious terminal text, absent browser storage/cookies, shell-only
  cache contents, and re-pairing after reload.
- Lost prompt response never triggers an application retry; possible duplicate
  command responses also remain ambiguous. New writes require explicit review
  after reconnect. Offline unpair deletes local data and reports unconfirmed
  server revocation. Hidden views make no polling/subscription requests.

Browser testing caught a native `fetch` receiver bug absent in Node, races from
obsolete refreshes/dialog events, and a Chromium lost-response path that surfaced
duplicate-command rejection. The adapter never retries mutations itself, but
browser/network behavior still requires the Engine's command-sequence/idempotency
fences. `command_sequence`, `input_unconfirmed`, and `outcome_unknown` are treated
as uncertain delivery. Old asynchronous refreshes cannot restore deleted data or
overwrite newer projections.

## Actual Rust gateway smoke

On 2026-09-08, `test/rust-smoke.mjs` passed with exit status 0 against #73's
compiled `zeus-companion` fixture example: a real local Rust Engine, real echo
PTY, auth enrollment store, and gateway. The committed JavaScript adapter was
used without compatibility changes. This closes the previously pending actual
gateway adapter smoke; it does not replace real iPhone/SSH/TLS validation.

Build the example from `zeus/` after integrating #73:

```sh
rtk cargo build --offline -p zeus-companion --example fixture
```

Run from the repository root with the absolute path to that example binary:

```sh
rtk proxy env COMPANION_RUST_FIXTURE=/absolute/path/to/zeus/target/debug/examples/fixture \
  node prototypes/companion-web/test/rust-smoke.mjs
```

The harness starts its own fixture. It creates an owner-only temporary directory,
reads the enrollment file without following a symlink, verifies ownership/mode,
keeps credentials out of argv/output, and only mutates the single known `fixture`
echo session. It holds the fixture's stdin open and closes it for teardown;
temporary enrollment files are deleted. It never attaches to a running user's
gateway. The optional command is separate from ordinary `npm test`.

The passing smoke verified:

- Expected server identity, pairing, hello, projects, and session discovery.
- Real screen projection and explicit mobile controller acquisition.
- Prompt echo in the PTY, increasing screen sequence, unchanged incarnation,
  and exactly one increment of the guarded command sequence.
- Authenticated WebSocket events and a rename-triggered invalidation.
- Reconnect preserving incarnation/command sequence and rejection of a duplicate
  prompt without another command-sequence increment.
- Confirmed live Archive and device self-revocation; an HTTP request using the
  retained, now-revoked credential returned 401, proving server-side rejection.

The smoke makes bounded screen reads to observe PTY echo. It does not separately
prove that delayed terminal output alone (without a session/status mutation)
produces a new invalidation. That remains a #73/#75 event-integration concern.
Its transport is the explicit loopback HTTP fixture exception; no deployment
TLS, private overlay, real SSH host, browser, or physical iPhone is involved.

## Numeric sample

Recorded 2026-09-07 on macOS arm64, Node v26.5.0. The fixture and client ran in
one Node process over loopback; there was no TLS, Engine, SSH, browser, or radio
in this measurement. Samples follow warm pairing/control acquisition. The
event sample measures invalidation delivery; the UI separately coalesces
invalidation-triggered reads over 150 ms.

| Operation | Samples | p50 | p90 |
| --- | ---: | ---: | ---: |
| Screen request and decode | 30 | 1.680 ms | 1.905 ms |
| Prompt acceptance plus fresh screen | 30 | 3.504 ms | 4.258 ms |
| WebSocket invalidation | 30 | 0.076 ms | 0.121 ms |
| Reconnect hello, list, and screen | 10 | 5.062 ms | 6.408 ms |

The entire sample transferred 4,074 request-body bytes, 44,039 response-body
bytes, and 2,294 event-body bytes, excluding headers/TLS/TCP overhead. Over one
second idle, the fixture/client issued zero requests and delivered zero events;
combined Node process CPU was 1,160 µs. Process RSS was 92,176,384 bytes, including
the fixture, Node runtime, and test instrumentation. These numbers do not measure
mobile memory, gateway costs, production latency, or long-term battery drain.

**Battery/radio energy and physical iPhone behavior are unmeasured.** Safari
VoiceOver, actual iOS Dynamic Type settings, installability, suspension, TLS over
a private overlay, and app-switcher screenshots need real-device validation.
The privacy curtain is best effort: a browser cannot guarantee when iOS captures
an app-switcher snapshot. Production remains no-go until that boundary is
resolved. The fixture's SSH-labelled row validates API/UI uniformity only;
actual SSH routing and PTY draining belong to #73/#75 integration tests.
