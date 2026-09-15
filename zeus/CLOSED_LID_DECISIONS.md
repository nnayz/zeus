# Closed-lid local execution: decision record for issue #70

## Status snapshot

This record covers the P0 investigation gate in `CLOSED_LID_PLAN.md` for
[issue #70](https://github.com/nnayz/zeus/issues/70). It does not approve an
implementation.

| Decision | Current status |
|---|---|
| Local macOS privilege/ServiceManagement architecture exception | **NOT APPROVED** |
| Privileged or real-`pmset` lab spike | **NOT APPROVED** |
| Shipment go/no-go | **NOT APPROVED** |
| Physical `pmset disablesleep` testing | **NOT RUN** |
| Physical ServiceManagement registration testing | **NOT RUN** |

Until the first two rows receive written approval, work is limited to documents,
mock backends, pure policy reducers, and tests that cannot change host power
state or register a privileged service. Shipment needs the later P6 decision;
approval of a lab spike would not approve shipment.

## Decision scope and repository facts

Issue #70 asks whether explicitly selected **local** sessions can continue to
make progress after a MacBook lid closes. A process merely surviving in suspended
RAM does not meet that promise. Remote sessions are separate: `REMOTE_PORT.md`
assigns remote persistence to the existing bootstrapped Remote PTY Holder. This
investigation must not change remote transport, SSH, or remote persistence.

The proposed local helper conflicts with the current repository rules in
`../AGENTS.md`, which prohibit elevation, host-wide configuration, and system
services. Therefore, a minimal local macOS power helper is an **architecture
exception**, not an application of an existing helper precedent. That exception
is **NOT APPROVED**. If it is later approved, the record must name the exact rules
being excepted and the narrow local-only replacement constraints before the
architecture documents are changed.

Relevant implementation facts, not proof that the feature is feasible:

- `crates/zeus-proto/src/model.rs` has `SessionRecord` and `SessionStatus`, but no
  closed-lid consent, local execution generation, or power lease model.
- `crates/zeus-engine/src/holder/protocol.rs` reports session ID, child PID,
  liveness, log/epoch offsets, optional Holder incarnation, and exact opaque
  Holder/child birth tokens (with legacy start fields retained). The exact
  identity fields can be absent for older Holders, and they
  do not by themselves encode consent or the Engine/helper lease identities.
- `crates/zeus-engine/src/registry.rs` can adopt live holders, respawn under an
  existing record, and hibernate/wake a process tree. Session ID alone therefore
  cannot prove one unchanged eligible execution.
- `crates/zeus-app/src/daemon_launch.rs` replaces a content-mismatched Engine and
  lets Holder-owned sessions survive for adoption. Any future lease design must
  explicitly handle both old and new Engine incarnations.
- `crates/zeus-updater/src/install.rs` uses an unprivileged detached `/bin/sh`
  app-swap helper. It is not a privileged installer or a ServiceManagement
  lifecycle implementation.
- `PACKAGING.md` describes a universal macOS 15+ desktop app, Developer ID and
  hardened-runtime signing, notarization, and no App Sandbox. Those facts do not
  establish that a power helper can be shipped or notarized.

## Research and provenance boundary

Aquarium is a **behavior-only reference** as described in issue #70. For this
record:

- Aquarium code provenance: **NONE**.
- No Aquarium source was copied, translated, vendored, or used as implementation
  input for these documents.
- The issue reports that Aquarium had no license when the issue was filed. This
  work did not recheck that fact and makes no current licensing claim.
- Any Zeus prototype must be independently authored unless maintainers first
  record a compatible license or written permission and all attribution duties.

The issue's description of `pmset -a disablesleep 1` is a candidate behavior,
not proof of a supported Apple API or a shipment commitment. No external or
primary-source mechanism research was performed for this record.

## P0 decision template

Copy and complete this section for a review. Do not replace evidence links with
verbal assurances.

```text
Decision ID: P0-CL-____
Issue: #70
Decision date: ____
Decision owner: ____
Security reviewer: ____
macOS/release reviewer: ____
Legal/provenance reviewer: ____

Scope requested:
  [ ] documents and mock-only work
  [ ] signed, isolated physical-lab spike
  [ ] other: ____

Architecture exception:
  Status: [ NOT APPROVED | APPROVED | REJECTED ]
  Exact ../AGENTS.md rules affected: ____
  Exact permitted component and authority: ____
  Why an unprivileged design cannot meet the user promise: ____
  Explicitly unchanged remote boundaries: ____
  Expiry/review date for the exception: ____

Mechanism decision:
  Candidate operation/API: ____
  Primary Apple source links and quoted support limits: ____
  Supported OS/CPU matrix proposed for testing: ____
  Distribution/notarization assessment: ____
  Undocumented-behavior stop condition: ____

Code provenance:
  Independently authored: [ YES | NO ]
  Aquarium classification: BEHAVIOR-ONLY
  Aquarium code used: [ MUST BE NO unless separately licensed and approved ]
  License/permission evidence, if any: ____
  NOTICE/attribution duties, if any: ____

Security and recovery:
  Threat-model revision/link: ____
  IPC identity and anti-downgrade design: ____
  Engine-proxy authorization design: ____
  Journal ownership and crash-point evidence: ____
  SleepDisabled coexistence disposition: ____
  Helper-death recovery disposition and measured bound: ____
  Manual recovery plan: ____

Product policy:
  Exact eligible local execution identity: ____
  Consent, expiry, restart, and re-arm semantics: ____
  AC/thermal/unknown-signal behavior: ____
  GUI quit and persistent override behavior: ____
  Conditional user-facing promise: ____
  Explicit non-goals and bag warning: ____

Evidence:
  Mock/fault-injection report: ____
  Signed ServiceManagement lab report: ____
  Physical closed-lid matrix: ____
  Update/rollback/uninstall report: ____
  Energy/wakeup measurements: ____
  Unresolved risks: ____

P0 outcome (lab spike only): [ NOT APPROVED | APPROVED | NO-GO ]
P0 rationale and stop conditions: ____
Approval signatures: ____

Separate P6 shipment outcome: NOT APPROVED
```

### Required interpretation

- P0 may authorize only the stated lab scope. It does not authorize a production
  implementation or shipping.
- A P0 approval must not silently weaken `REMOTE_PORT.md` or create a remote
  architecture exception.
- P6 must choose `GO`, `NARROWER RE-SPIKE`, or `NO-GO` only after P1-P5 evidence
  is attached. Until then, the shipment state remains **NOT APPROVED**.

## Proposed policy decisions for a future spike

These are hypotheses to test, not accepted product requirements:

1. The feature is off by default. Helper installation is not session consent.
2. Consent names exact live local held executions and expires at the earlier of
   execution end or a maximum duration. It does not follow a respawn, wake,
   adoption, migration, reboot, or changed process/Holder identity.
3. Status `Working` alone is not eligibility. Long child commands, `Idle`, and
   `NeedsInput` can remain eligible when exact execution identity, consent,
   liveness, and safety are still valid.
4. Remote, direct, deferred, archived, exited, hibernated, or unverifiable
   executions are ineligible in a proposed v1.
5. V1 is AC-only. Missing, stale, or inconsistent power, battery, lid, thermal,
   helper, or identity evidence revokes or blocks acquisition.
6. Serious or critical thermal pressure, AC removal, explicit hibernation,
   duration expiry, identity change, and **Allow Sleep Now** release all grants.
   Re-arm requires a new explicit action.
7. GUI quit is not allowed to leave an invisible active policy. Either the Engine
   keeps a truthful persistent indication and reachable override, or it releases
   and verifies restoration before GUI exit. The product choice remains open.
8. Readiness wording must be conditional and evidence-backed. It must never
   promise survival through emergency sleep, shutdown, reboot, a mechanism
   removed by macOS, or use in an enclosed bag.

## Proposed numerical budgets

Every value below is a **PROPOSAL REQUIRING MEASUREMENT** on the approved
platform matrix. None is an achieved result or shipment commitment. P0/P3/P5
may replace values, but must record the workload, clock, percentile, worst case,
and reason.

| Item | Initial proposal | Measurement/gate |
|---|---:|---|
| Engine renewal interval | 15 s | Steady-state trace; no polling of processes |
| Helper lease TTL after last valid renewal | 45 s | Fault-injected Engine death; monotonic clock |
| Maximum one-time consent duration | 8 h | Expiry and user-comprehension tests |
| Power/thermal/lid evidence freshness | 5 s | Timestamped event delay plus injected event loss |
| Fixed power-operation timeout | 2 s | Hung-command injection; no overlapping invocation |
| AC-removal to verified restoration | <= 5 s | Physical attended lab, worst case |
| Serious/critical thermal to verified restoration | <= 5 s | Supported signal injection plus physical confirmation where possible |
| Engine-death to verified restoration | <= 60 s | TTL plus operation/readback, repeated trials |
| Helper restart recovery after an ordinary crash | <= 60 s | Signed lab service, including launch throttling |
| Inactive recurring helper wakeups | 0/min after recovery settles | Instruments/powermetrics-equivalent approved method |
| Active renewals | <= 4/min | IPC trace |
| Inactive helper CPU | <= 0.1% of one core | 30-minute baseline comparison |
| Active control-plane CPU | <= 0.5% of one core | Same workload with feature off/on |
| Helper resident memory | <= 20 MiB | Peak and steady-state measurements |

The 60-second helper-recovery row is only a desired test gate. It is not currently
enforceable by the proposed lease. See the blocker below.

## Mandatory no-go blocker: global boolean and helper death

`SleepDisabled` is a system-wide boolean, not an owned or expiring assertion.
Another writer can set the same boolean, and before/after readback cannot prove
which writer owns it. More critically, if the privileged helper dies while the
boolean remains set, the Engine's lease expiry cannot execute restoration. A
short lease therefore does **not** bound helper-death cleanup.

This is an explicit **NO-GO BLOCKER** unless the approved design can demonstrate
both of the following without adding an unreviewed privileged watchdog:

1. Zeus never silently overwrites or clears another utility's state, including
   all observable races accepted by the review; and
2. helper death, crash loops, launchd throttling/disablement, corrupt or missing
   recovery state, update interruption, and uninstall yield verified restoration
   within an approved measured bound or a deliberately approved fail-safe result.

A warning, best-effort restart, manual recovery, journal alone, or successful
command exit is not enough. If deterministic ownership and bounded cleanup
cannot be made trustworthy, P6 must record **NO-GO**.

## Evidence matrix

`NOT RUN` means no claim has been validated by execution.

| Question | Current evidence | Status | Evidence required before P6 |
|---|---|---|---|
| User problem and candidate mechanism | Issue #70 and `CLOSED_LID_PLAN.md` | DOCUMENTED, NOT VALIDATED | Primary-source review and physical matrix |
| Current privilege conflict | `../AGENTS.md` | CONFIRMED BY REPO POLICY | Written narrow exception or no-go |
| Remote scope is separate | `REMOTE_PORT.md`; issue #70 | CONFIRMED BY REPO DESIGN | Regression review showing no remote changes |
| Session/status model lacks proposed consent/generation | `crates/zeus-proto/src/model.rs`; pure `zeus-power` policy core | MODEL AND UNIT TESTS COMPLETE; ENGINE CONSENT NOT INTEGRATED | Engine coordinator, caller authorization and UI policy after approval |
| Adoption/respawn/hibernate affect identity policy | Exact Holder/child birth-token and identity-pinned adoption fixtures | MOCK/LOCAL LIFECYCLE TESTED | Engine restart/upgrade adversarial soak and physical matrix |
| Lease/recovery semantics | `zeus-power` bounded codec, auth binding, lease reducer and fake recovery backend | PURE MODEL FAULT-TESTED; NO PLATFORM BACKEND | Durable journal, signed IPC, helper-death/global-state resolution |
| Existing updater is not helper lifecycle support | `crates/zeus-updater/src/install.rs` | CONFIRMED BY SOURCE REVIEW | Signed install/update/rollback/uninstall rehearsal |
| Desktop packaging baseline | `PACKAGING.md`; `scripts/package.sh` | CONFIRMED BY REPO DOCS | Nested helper signing and notarized artifact tests |
| `pmset disablesleep` works across macOS 15+ and both CPUs | None in this work | **PHYSICAL TEST NOT RUN** | Attended Apple silicon and Intel test records |
| ServiceManagement registration/authenticated IPC works | None in this work | **PHYSICAL TEST NOT RUN** | Signed isolated lab prototype and adversarial tests |
| Same process makes progress with lid closed | None in this work | **PHYSICAL TEST NOT RUN** | PID plus start-identity, timestamped progress soak |
| Ordinary lid sleep returns after release/failure | None in this work | **PHYSICAL TEST NOT RUN** | Release, crash, reboot, and removal trials |
| Proposed latency/energy budgets are met | None in this work | **NOT MEASURED** | Recorded methods, raw results, worst case and percentiles |
| Aquarium implementation is reusable | No code/license research in this work | NOT ESTABLISHED | Independent implementation, or separate license approval |

## Mock and physical-lab separation

### Allowed before architecture approval

- Pure eligibility reducer with fake session identities and fake clocks.
- Mock helper protocol, authentication failures, bounded-codec fuzzing, and
  Engine-proxy authorization tests that cannot reach a power command.
- In-memory/file-fixture journal crash points inside an unprivileged temporary
  directory.
- Packaging metadata design and static inspection.
- UI copy and state mockups with an unmistakable fake-backend marker.

Mock success proves only policy/state-machine behavior. It cannot support “Safe
to close,” platform support, bounded cleanup, ServiceManagement feasibility, or
shipment.

### Requires separately approved physical lab scope

- Any `pmset` read or write used as feature evidence.
- ServiceManagement registration, privileged identity checks, helper crash loops,
  upgrade, rollback, or uninstall.
- AC removal, thermal handling, lid closure, reboot, energy, or wakeup tests.

Physical tests must use a dedicated, attended, ventilated Mac, a separate test
signing identity and state namespace, recovery access, recorded initial/final
power state, and a reviewed cleanup checklist. They must be explicit opt-in and
must never run in ordinary CI or on a developer's active machine. These tests are
**NOT RUN**.

## Current conclusion

P0 architecture exception: **NOT APPROVED**. Privileged lab work: **NOT
APPROVED**. Shipment go/no-go: **NOT APPROVED**. The required next action is
maintainer/security/legal review of the P0 template and the no-go blocker, not a
power-state prototype.
