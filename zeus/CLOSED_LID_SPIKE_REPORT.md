# Closed-lid execution spike report — #70

## Outcome of this draft

This branch executes the parts of `CLOSED_LID_PLAN.md` that are allowed by the
current repository architecture: source review, independent threat modeling,
exact local execution identity, pure eligibility/authentication/lease/recovery
models, fault-injection tests, and read-only package validation.

It deliberately does **not** add a privileged executable, ServiceManagement
metadata, a `pmset` backend, installation UI, or a user-visible **Safe to close**
claim. After this mock spike was completed, the solo repository owner approved
the narrow production-targeted exception in `../AGENTS.md` on 2026-09-16.
Implementation and dedicated physical-lab work may proceed on a separate branch.
Shipment is conditionally authorized but cannot become an effective `GO` until
all recorded release gates pass.

No Aquarium source was read, copied, translated, vendored, or used to author the
code in this branch.

## Delivered artifacts

| Plan area | Delivered in this branch | Current limit |
|---|---|---|
| P0 decisions/provenance | `CLOSED_LID_DECISIONS.md`, local SDK observations in `CLOSED_LID_EVIDENCE.md` | Maintainer/security/legal lab approval absent; web/current licensing review not performed |
| Threat boundary | `CLOSED_LID_THREAT_MODEL.md` with actors, two-hop authorization, global-setting and helper-death blockers | No platform audit-token or code-signing adapter exists |
| Exact local identity | Additive session ID, Holder incarnation, and opaque Holder/child birth tokens; fresh PID-reuse checks; identity-pinned adoption; `Session::local_execution_identity` tests | Older Holders and any missing observation remain usable but power-ineligible; host boot identity and consent are not integrated into Engine state |
| P1 protocol/auth model | `zeus-power::wire` bounded fixed v1 messages; `auth` exact role/UID/executable/team/requirement/build/protocol/capability policy and replay/incarnation binding | Trusted facts are supplied by a future platform adapter; this is not authenticated macOS IPC |
| P2 eligibility | Pure reducer accepts only an explicitly selected, exact, live, awake local Held execution; status is diagnostic only | No GUI action, persisted consent, Engine coordinator, or governor integration is exposed |
| P3 lease/safety | Monotonic aggregate coordinator, caps, sticky Allow Sleep Now/safety trips, AC and fresh sensor requirements | Observations are test inputs; no IOKit/power/thermal source is connected |
| P3 recovery | Prepared/Owned/Restoring state machine and fake semantic backend with injected failure tests | No root-owned durable journal, exclusive Helper lifetime, live-incarnation proof, or system mutation; global boolean/helper-death blockers unresolved |
| P4 release | `CLOSED_LID_RELEASE_SPIKE.md` and read-only `scripts/check-power-helper-package.sh`; CI self-test | Current package correctly reports helper unavailable; no signed helper, registration, updater transaction, or uninstall path |
| P5 physical/performance | Required matrix and proposed budgets documented | No physical closed-lid, Intel, macOS 15+, ServiceManagement, energy, or recovery measurement was run |
| P6 decision | Explicit evidence gaps and stop conditions | Cannot make a shipment go/no-go until the blocked physical/security work is authorized and completed |

## Test evidence on this branch

Safe automated checks run during development:

```sh
cargo test -p zeus-power
cargo clippy -p zeus-power --all-targets -- -D warnings
cargo test -p zeus-engine holder::protocol::tests --lib
cargo test -p zeus-engine --test holder   a_holder_owns_a_session_end_to_end -- --nocapture
cargo test -p zeus-engine --test holder_session   a_held_session_survives_its_session_object_and_is_adoptable -- --nocapture
bash -n scripts/check-power-helper-package.sh
bash scripts/check-power-helper-package.sh --self-test
```

The power-core suite covers:

- oversized, truncated, wrong-version, wrong-kind, unknown-tag, excessive-list,
  invalid-identity, and trailing-byte frames;
- missing/wrong transport, audit-token validation, code signature, designated
  requirement, role, UID, executable ID, Team ID, Build ID, protocol and capability;
- replay and stale Engine/helper/channel incarnation rejection plus response binding;
- remote/deferred/direct, unselected, hibernated, unknown/dead, PID-birth and
  route/identity mismatch eligibility;
- Working, Idle, waiting, finished, failed and unknown reduced status without
  using display state as liveness proof;
- missing/stale/future safety observations, battery, thermal, helper-health,
  emergency state, clock regression, deadline caps, sticky override and re-arm;
- acquisition/release failures before and after side effects, startup recovery,
  conflict, corrupt or unsupported journal, external state changes, different
  owner, verified release, and explicit non-restoration of ambiguous Prepared
  state using only the fake backend.

The package self-test covers missing helper, a mocked valid contract, symlink,
missing/extra architecture, signature, Team ID, designated-requirement and
hardened-runtime failures. It does not simulate Apple security services.

Final workspace command results must be copied into the draft PR after they run.

## Review gates before any next code phase

This branch remains a **non-privileged exploration**. The owner-approved issue #70
exception now permits a separate production-targeted follow-up while leaving
`REMOTE_PORT.md` behavior unchanged. That follow-up must remain mock-first and
must keep real effects restricted to explicit, attended physical-lab runs:

1. package a separately identified test helper and ServiceManagement plist;
2. extract peer audit-token facts and evaluate the real code signature at the
   connection/consumption boundary;
3. keep the real power backend disabled while registration, identity, replay,
   update and removal behavior is tested;
4. authorize fixed real operations only after the journal permissions,
   interruption harness, manual recovery and attended hardware protocol pass
   security review; and
5. prove exclusive Helper execution and distinguish live same-installation leases
   from stale recovery state; and
6. stop with no-go if helper death or external-global-state races cannot meet the
   approved cleanup/coexistence standard.

Even a successful lab does not authorize UI readiness wording or shipment. Those
require the full physical matrix, energy measurements, signed lifecycle evidence,
and a separate P6 decision.
