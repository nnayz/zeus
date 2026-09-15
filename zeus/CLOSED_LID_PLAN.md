# Controlled local execution with the lid closed — #70

## Status and scope

Consolidated exploration plan for [#70](https://github.com/nnayz/zeus/issues/70).
**Proposed; no privileged-helper implementation or architecture exception is
approved by this document.** Complete the investigation and record a go/no-go
before opening a production implementation issue.

Execution artifacts:

- [decision record](CLOSED_LID_DECISIONS.md);
- [threat model](CLOSED_LID_THREAT_MODEL.md);
- [read-only SDK evidence](CLOSED_LID_EVIDENCE.md); and
- [release/package spike](CLOSED_LID_RELEASE_SPIKE.md).

The goal is explicit, per-session permission for selected **local** sessions to
make progress while a MacBook lid is closed. Process survival in suspended RAM
is not execution. Ordinary idle-sleep assertions / `caffeinate` do not establish
this promise. The issue identifies undocumented `pmset disablesleep` behavior
as a candidate; its safety and suitability for distribution remain unverified.

Remote Holders already provide a separate persistence path on compatible hosts.
Do not change `zeus-remote`, SSH, the Remote Holder protocol, or remote persistence.
[#40](https://github.com/nnayz/zeus/issues/40) owns notification policy and delivery;
this work neither implements phone alerts nor makes their delivery a prerequisite.

No general process allowlists, session supervisor, private brightness APIs,
Swift daemon, battery mode in v1, or guarantee against emergency sleep, shutdown,
reboot, or execution inside a bag. Preserve normal Zeus behavior when this
optional feature is absent, disabled, incompatible, unhealthy, or unsupported.

## 1. Constraints and decisions to close

`../AGENTS.md` prohibits elevation, host-wide configuration, and system services.
`REMOTE_PORT.md` is the active remote baseline; `PLAN.md` is historical, not
permission to introduce a different architecture. Stop at that conflict:
maintainers must explicitly approve a narrowly scoped **local macOS power**
exception before any privileged prototype. Never broaden the remote exception
because there is none. Record the approval and exact affected rules; update
`AGENTS.md` and the relevant architecture boundary in `REMOTE_PORT.md` only when
approved, without changing remote behavior.

| Decision | Proposed direction, not yet approved | Evidence required to close |
|---|---|---|
| Mechanism and platform support | Investigate fixed `pmset` operations; fail closed outside a verified matrix | Primary Apple documentation/SDK review, macOS 15+ physical-Mac tests on Apple silicon and Intel, behavior on OS updates, distribution/notarization review |
| Licensing | Independently authored Rust; do not copy, translate, vendor, or derive Aquarium source | Record provenance. The issue reports Aquarium had no license when filed; recheck before any reuse. Compatible license or written permission plus license obligations and `NOTICE` attribution are required for derived code |
| Privilege boundary | Optional minimal Rust helper, not another Engine or Holder | Approved local-only exception, reviewed threat model and authenticated IPC prototype |
| Eligibility | Explicit selected-session lifetime, bounded by a maximum duration, not `Working` alone | Lifecycle/status fixtures, long child-command tests, consent and energy review |
| Safety | AC-only; unknown safety data disables operation; serious/critical thermal pressure releases | Verified signal sources, freshness rules, and measured reaction bounds |
| Ownership and recovery | Expiring Engine lease plus durable root-owned recovery journal | Demonstrated restoration after failure and an explicitly accepted coexistence limitation; otherwise no-go |
| Installation and update | Supported ServiceManagement registration, signed universal bundled helper | Signed registration/IPC proof and complete update/rollback/uninstall rehearsal |
| User promise | Conditional readiness, never a guarantee of uninterrupted execution | Reviewed wording and tests that remove readiness on stale or failed evidence |

Aquarium is a behavior reference only: <https://github.com/ZimengXiong/aquarium>.
Do not treat its implementation as licensed input or its observed behavior as an
Apple support commitment. Notarization success alone does not settle whether an
undocumented power mechanism is acceptable to ship.

## 2. Candidate architecture and trust boundary

```text
Desktop: consent, selected sessions, status, Allow Sleep Now
    -> local Rust Engine: session identity, eligibility, bounded renewal
        -> authenticated, versioned local IPC
            -> optional zeus-power-helper: lease, safety, owned-state recovery
                -> fixed macOS power operations
```

All implementation would live under the Rust workspace. A candidate
`crates/zeus-power-helper` must be macOS-only and independent of GPUI, terminal
parsing, Agent manifests, and remote transport. The Engine owns session policy;
the helper owns only the minimal system-wide power decision and cleanup facts.
It must never enumerate processes to discover eligible Agents.

Investigate ServiceManagement / `SMAppService` registration and an IPC transport
that exposes an unforgeable peer audit token to Rust (for example, XPC through
system bindings). Do not select a generic socket unless it can meet the same
identity requirements. No shell installer or arbitrary root command channel.

Candidate bounded methods: `status`, `acquire`, `renew`, `release`, and
`prepare_uninstall`. Negotiate protocol, exact helper Build ID, and capabilities
before acquisition. No caller-supplied executable, arguments, paths, environment,
shell text, or `pmset` values. If commands are needed, use only fixed absolute
executables and internally fixed argument vectors; bound runtime and verify the
actual resulting state, not only the command exit status.

Authentication must cover **both directions**. Bind each connection/lease to the
peer audit token, permitted executable identifier and designated requirement,
Developer ID team, explicitly authorized UID, supported build/protocol, Engine
incarnation, helper boot nonce, and monotonic request sequence. Reject unsigned,
wrong-team, wrong-user, differently signed, downgraded, replayed, stale, oversized,
and malformed requests. A claimed PID, path, UID, version, or bundle identifier
in a request is not authentication. Prove how an old correctly signed executable
is excluded; a Team ID check alone does not enforce a minimum build.

The threat model must also follow the full desktop -> Engine -> helper path:
an arbitrary local program must not gain power control by asking a genuine
Engine to proxy its request. Define authorization for enabling, selecting,
renewing, revoking, and reading status. Default to one explicitly authorized
local UID and one Engine lease; reject other users, including fast-user-switch
cases, until policy is explicitly defined. The helper independently enforces
safety limits and lease duration even for an authenticated Engine.

Root policy/recovery directories must be root-owned `0700`, files `0600`, with
bounded sizes, atomic durable writes, schema versions, and owner/type/symlink
checks. Do not trust user-writable bundle paths or state when running as root.
Logs contain reason codes and minimal identities, never prompts, terminal
contents, credentials, environments, or Agent command lines. Rate-limit failed
requests and bound connections, memory, and command execution.

## 3. Current integration points and gaps

These are implementation seams to investigate, not approval to modify them:

| Area | Current Rust path / behavior | Planning consequence |
|---|---|---|
| Session model | `crates/zeus-proto/src/model.rs`: `SessionRecord`, `SessionStatus`; no local runtime incarnation or power consent | Separate requested/eligible/confirmed power state from display status; add versioned identity/consent only after approval |
| Local execution identity | `crates/zeus-engine/src/session.rs`: deferred spawn and `adopt_with_status`; `holder/protocol.rs`: `HolderStat` has PID/alive and optional output epoch, not a stable incarnation | A record can precede a child. Output offsets and PIDs are not sufficient identity. Investigate child birth identity plus Holder incarnation; older/unverifiable holders remain usable but ineligible |
| Lifecycle | `crates/zeus-engine/src/registry.rs`: `restore`, `adopt_live_holders`, `respawn`, `hibernate`, `wake_session` | Respawn reuses Session ID. Hibernate stops the tree without exiting the Holder. Revoke consent by execution generation, not merely record ID |
| Status/governor | `crates/zeus-engine/src/status/mod.rs`: `StatusReducer`; `governor.rs`: idle/memory hibernation | Quiet Working can become Unknown, and Idle is not command completion. Decide power-mode precedence over idle hibernation; resource/safety overrides remain explicit |
| Engine coordination | `crates/zeus-engine/src/bin/zeusd-rs.rs`, `control.rs`, `events.rs` | Coordinator belongs in Engine startup/lifecycle, not a Holder. Reconcile authoritative state after event gaps; avoid helper IPC under the Registry lock |
| Client and UI | `crates/zeus-client/src/client.rs`; `crates/zeus-app/src/store/mod.rs`, `session_surfaces.rs`, `surface_shell.rs`, `macos/menu_bar.rs` | Add typed actions and effective-state snapshots. The menu bar belongs to the app and disappears on exit; app-independent indication is not implemented |

## 4. Proposed Engine eligibility and user policy

The spike should test this conservative, understandable default before approval:

- Feature off by default. Installation consent is not session consent.
- User opts in to exact live local **held** executions for a bounded duration.
  Direct, deferred, archived, or unverifiable executions are ineligible in the
  proposed v1. Selection does not carry into a respawn, reopen, migration, new
  incarnation, machine boot, or sibling/MCP-created session. Child processes in
  the selected execution are covered; define the detached/reparented-work boundary.
- Explain that sleep control is **system-wide**: selection determines Zeus consent,
  but all local processes can run while the machine stays awake. This is not
  per-process power isolation.
- Track Session ID, execution location, Holder/child identity and process start
  identity, Engine incarnation, hibernation/liveness facts, and reduced status.
  PID alone cannot identify a session. Unknown or inconsistent identity denies
  eligibility; remote, exited, hibernated, or revoked sessions cannot renew.
- Prefer **until selected session exits or duration expires**, rather than
  `Working`-only. Idle/Done reduction can be transient and a long child command
  must not lose protection because of a screen heuristic. Screen-status Unknown
  alone need not revoke independently verified execution liveness; unknown
  identity or safety evidence must. Permission/Question (NeedsInput) remain
  eligible within the same cap; show that they are waiting and still
  holding the Mac awake. Releasing on those statuses is a separate policy choice,
  not an assumption about notification delivery from #40.
- Aggregate eligible sessions into one Engine lease. Removing the last one
  releases it. Exit, hibernation, migration away from local execution, identity
  change, cancellation, expiry, and failed adoption revoke eligibility.
- Propose exemption from idle auto-hibernation only during the bounded consent
  window, without secretly pinning the session. Explicit hibernate and safety
  overrides win. Resolve memory-governor precedence and show any resource-driven
  revocation. Attach/wake/queued input must not silently re-arm consent.
- Engine restart/adoption cannot replay a lease. Revalidate exact live identities
  and consent against the new Engine/helper handshake. Default to fresh user
  consent after restart for v1; never guess from a persisted PID or status.
  Any proposal to retain consent must prove unchanged execution identity and
  preserve the original deadline.
- GUI quit does not revoke an otherwise eligible Engine-owned lease. But active
  indication and a reachable **Allow Sleep Now** action must remain available.
  Resolve app-quit/menu-bar lifecycle explicitly; if indication cannot remain,
  tell the user and release before quit rather than silently running invisibly.
  The issue also asks for restoration after app crash within a lease bound.
  Intentional quit vs crash needs an explicit Engine contract; do not claim both
  behaviors are satisfied merely because the Engine survives either event. A
  release-on-quit fallback changes the proposed product behavior and needs review.
- **Allow Sleep Now** revokes all selected-session grants and verifies restoration.
  It must not be immediately undone by automatic renewal. Safety cutoffs clear
  active grants; require explicit re-arm after safe conditions return in v1.

Require AC, readable/fresh power source, battery, lid, thermal and helper state,
and a supported Mac/OS configuration. Define how to handle unknown sensors and
machines without a battery rather than assuming they are safe. Keep the internal
display free to turn off; do not acquire display-wake assertions or manipulate
brightness. Serious/critical thermal pressure, AC removal, unknown/stale safety
state, or a maximum-duration limit releases the lease. Never disable macOS
emergency protection. Battery operation is deferred; any later proposal needs
advanced consent, hysteresis, a non-disableable emergency floor, and separate tests.

UI states should distinguish unavailable, authorization required, ready to arm,
active/confirmed, waiting, expired/revoked, conflict, and restoration failed.
Only show **Safe to close** (or approved less absolute wording) after helper
identity, current lease, exact session eligibility, power-state readback and all
safety prerequisites are confirmed. Expire that indication when its evidence
expires. State that this is for a ventilated desk, **not a bag**. Explain why
protection ended; do not claim Zeus paused/stopped an Agent unless it actually did.
For restoration failure, do not claim normal sleep has returned.

## 5. Lease, global ownership, and recovery investigation

Candidate state machine:

```text
Unavailable / Disabled -> Authenticating -> CheckingSafety
    -> Armed (journal durable + state change verified + lease valid)
    -> Releasing -> Disabled (restoration verified)
Any uncertainty -> Blocked / RecoveryRequired (no new lease, no readiness claim)
Helper startup -> RecoverBeforeAcquire -> Disabled or RecoveryRequired
```

Define numerical budgets before prototype acceptance: renewal period, maximum
lease TTL, maximum session duration, safety-signal freshness, command timeout,
Engine-death restoration, helper-death recovery, and AC/thermal reaction time.
Measure worst case as well as typical timing; no unspecified "short lease" is a
release criterion. Use a monotonic deadline with explicit suspend/wake behavior,
boot identity, and no lease revival after reboot or wall-clock change.

Acquisition records the observed prior state and intent durably **before** a
possible system mutation; records ownership only with verified evidence. The
journal must model each crash point, including write-intent -> command -> readback
-> commit and restore -> readback -> journal removal. Recovery handles missing,
corrupt, unsupported, or partially written records before accepting new leases.
Do not turn ambiguous ownership into permission to blindly write a default.

`SleepDisabled` is a system-wide boolean, not a tokenized assertion. Checking it
before and after a write cannot detect another utility writing the same value.
Proposed v1 policy: refuse acquisition if already disabled or ownership is
uncertain; detect observable external changes, stop renewal, and report conflict.
Restoration must account for external takeover, not simply force `0` on every
failure. Even this policy **cannot be race-free** with another writer. Document
and review the remaining limitation (potentially requiring exclusive use);
if "do not clobber externally controlled state" cannot be met to the approved
standard, stop with no-go. A warning alone is not proof of correct ownership.

A lease is not self-enforcing after the helper dies: the global bit can outlive
the process that was supposed to clear it. Prove supervised restart and recovery
under launchd crash throttling, repeated failure, disabled registration, and
interrupted removal. A missing/corrupt journal may make safe restoration
ambiguous. Do not promise a bounded helper-death recovery until this is resolved;
do not silently add a second privileged watchdog to cover the gap. Any extra
privileged component needs a new architecture review. Manual recovery is necessary
but is not a substitute for the crash-cleanup acceptance gate.

## 6. Packaging, update, and removal design

The desktop baseline is macOS 15+, universal Apple silicon/Intel, Developer ID,
hardened runtime, notarization, no App Sandbox (`PACKAGING.md`). This **local**
helper matrix must not expand the remote matrix (macOS arm64 only).

Current integration seams are `scripts/package.sh` (universal binaries, nested
signing, app-first notarization), `assets/zeus.entitlements`, and
`crates/zeus-updater/src/install.rs` (`launch_installer` / `installer_script`,
unprivileged shell-based app swap and rollback). `../Casks/zeus.rb` currently
installs the app and defines user-data cleanup, but has no privileged-service
uninstall flow. None of these is already a power-helper lifecycle implementation.
Add explicit package/signing/CI coverage for the helper and ServiceManagement
metadata, including signing nested binaries individually before signing the app.

The current updater waits for the GUI PID, not Engine/helper quiescence. Its
rename/copy rollback on ordinary copy failure is not an atomic transaction or
proof of recovery from power loss between steps. Test those interruption points.
Also cover `crates/zeus-app/src/daemon_launch.rs`: bundled Engine hash changes
trigger Engine replacement while Holders survive. Both an old Engine still
running during the swap and its subsequent replacement must obey lease revocation.

Design the whole lifecycle before treating an authenticated `pmset` call as success:

1. Bundle the exact signed universal helper and registration metadata; never
   download a helper selected at runtime. Explain root scope, global power effects,
   safety limits and removal before explicit ServiceManagement authorization.
   Re-authenticate the actual helper consumed at the privileged boundary; earlier
   updater verification of a user-writable staged bundle is not sufficient.
   Prevent path substitution and verification-to-execution races, and pin the
   consumed executable identity/build, not merely its expected pathname.
2. Validate registration states (not installed, pending approval, enabled,
   disabled, incompatible), installed identity and exact Build ID/protocol.
   Failure disables this feature only, not ordinary session execution.
3. Coordinate updates with the existing unprivileged updater: release the lease,
   verify owned-state restoration, quiesce acquisitions, replace/register through
   the supported flow, verify the new helper, then require a fresh handshake.
   The existing generated shell swap helper must never become a root installer.
4. Specify old-app/new-helper and new-app/old-helper behavior, app relocation,
   duplicate app copies, Engine survival/replacement, interrupted registration,
   rollback, and authorization denial. Prefer unavailable over compatibility
   shortcuts. Rollback must not let an older signed client bypass the build floor;
   specify how safe removal remains possible with incompatible clients.
5. Uninstall restores and verifies owned state before unregistering components.
   Test app deletion and Homebrew removal paths as well as the explicit UI.
   If removal fails, retain actionable recovery evidence, not a success message.
6. Publish a reviewed manual recovery runbook: identify registration and power
   state, stop renewal, distinguish Zeus ownership from another utility, restore
   only with informed authorization, verify ordinary lid sleep, remove components.
   Validate exact commands during the spike; do not provide a blind reset script.
7. Ad-hoc/dev builds use a mock backend. Any signed privileged lab prototype needs
   a separate test identity, registration and state namespace on a dedicated Mac.
   No environment variable may bypass release helper authentication.

## 7. Workstreams, dependencies, and stop/go gates

No changes in this plan enable power control on a developer's current machine.
Assign named owners and reviewers when scheduling the workstreams below.

| ID / suggested owner | Work and deliverable | Depends on / exit gate |
|---|---|---|
| P0 — maintainer + security/legal | Decision record: local privilege exception, independent-code provenance, primary-source mechanism review, supported test matrix, user promise, explicit stop conditions | First. Written approval for the limited lab spike; not shipping approval |
| P1 — macOS/security | Rust ServiceManagement and authenticated IPC lab prototype; full trust-boundary diagram and adversarial tests, including Engine proxy authorization | P0; no unauthorized client can affect even the mock power backend |
| P2 — Engine | Pure eligibility reducer and lifecycle/consent fixtures; decide status, adoption, app-quit indication, duration and re-arm semantics | Can research/mock alongside P1 after P0; identity and policy review complete |
| P3 — macOS/reliability | Fixed-operation backend, safety notifications, lease/journal state machine, ownership/conflict analysis, fault-injection report | P1 plus reviewed P2 policy; lab authorization before real power mutation; bounded cleanup and coexistence disposition |
| P4 — release/security | Signed universal packaging, registration, upgrade/rollback/uninstall prototype and manual recovery runbook | Design parallel with P1/P2; final rehearsal depends on P3; notarized lifecycle and downgrade tests pass |
| P5 — UI/performance/QA | State/consent mockups, physical-Mac matrix, energy/wakeup report, same-process progress soak, failure evidence | Mockups early; hardware tests after P3/P4; all acceptance rows below have evidence |
| P6 — maintainers | Consolidated decision with reviewers, measured budgets, supported versions, residual risks, rejected alternatives and evidence links | P1–P5 complete; explicit go, narrower re-spike, or no-go |

P6 go authorizes creation of a **separate** implementation issue, not immediate
shipping. Split that issue by the same ownership boundaries with concrete
acceptance tests. Record approved policy, IPC/schema versions, deployment matrix,
and cleanup bounds in architecture/packaging docs. No-go retains ordinary
lid-open idle-sleep prevention where supported and persistent remote execution
as alternatives; do not label either as local closed-lid execution.

## 8. Validation and completion checklist

Automated tests use fake power backends, fake clocks, fixtures and bounded IPC;
no default test may install a service or mutate host power state. Physical tests
are explicit opt-in on a ventilated, attended lab Mac with recovery access.

| Gate | Required evidence |
|---|---|
| Trust and IPC | Unsigned, wrong-team/executable/UID, second-user, old signed build, replay, bad nonce/incarnation, oversized/malformed/flooded requests rejected; no arbitrary command/value/path surface; both IPC hops tested |
| Durable state | Unprivileged mutation, symlink/type/ownership attacks, missing/corrupt state, disk-full and interruption at every journal/write/restore boundary; deterministic outcome or explicit no-go |
| Eligibility | Local vs remote, PID reuse, long command with Idle reduction, Permission/Question, multiple selected sessions, last-session exit, hibernation/wake, revocation, incarnation change, Engine adoption and GUI quit |
| Power and ownership | Existing disabled state, concurrent same/different-value writers, unknown/stale sensors, AC removal, serious/critical thermal signals, rapid power transitions, emergency sleep and unsupported OS behavior |
| Failure bounds | Engine/app crash, helper SIGKILL/restart/crash loop, launchd disable/throttling, sleep/wake, hibernation, reboot, command hang/failure; measured restoration vs each agreed bound, never only loss of lease |
| Release lifecycle | Developer ID/designated requirements, both CPU slices, hardened runtime, notarized app/zip, registration approval denial, upgrade version pairs, interrupted swap, rollback, uninstall/removal and manual recovery |
| Physical behavior | For each supported macOS/architecture entry, record model/OS/build IDs and power state; prove same Agent PID **and start identity** with time-stamped progress while lid is closed; after release prove ordinary lid sleep; include forced Engine/helper death, reboot, low-power transition and removal |
| UI truthfulness | No readiness before verified activation; stale evidence clears it; visible active state and override survive supported GUI lifecycle; conflicts/cutoffs/restoration failures explain actual state; bag warning and conditional promise reviewed |
| Performance | Compare inactive baseline and active lease with the same workload: CPU, memory, energy, wakeups, renewal rate and safety latency; no process scans, event-driven safety/session updates, bounded renewals/expiry, no inactive recurring work beyond necessary recovery |

Before P5, record exact numerical budgets and the test/measurement method for
every timing and overhead claim. Prefer notifications over polling; any required
fallback polling needs measured justification. Do not satisfy energy targets by
weakening identity, cleanup or emergency-safety behavior.

For later Rust prototypes/implementation, run narrow relevant tests first, then
`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, and `cargo build --workspace --release` when practical.
These checks do not replace signed-package tests or physical closed-lid soaks.

Issue #70 is complete when P6 records a reviewed go/no-go and links the provenance,
threat model, prototype results, policy decisions, lifecycle runbook, measured
energy/recovery report, and platform evidence. It is not complete merely because
one Mac stayed awake in a demo.
