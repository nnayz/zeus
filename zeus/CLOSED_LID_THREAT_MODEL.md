# Closed-lid local execution: threat model for issue #70

## Status and scope

This is a planning threat model for [issue #70](https://github.com/nnayz/zeus/issues/70)
and `CLOSED_LID_PLAN.md`. It describes a possible local macOS-only power helper.
It does not treat any mock or parallel spike as evidence of a privileged helper,
physical power behavior, or ServiceManagement feasibility.

- Architecture exception: **NOT APPROVED**.
- Privileged/ServiceManagement lab spike: **NOT APPROVED**.
- Shipment go/no-go: **NOT APPROVED**.
- Physical `pmset` tests: **NOT RUN**.
- Physical ServiceManagement tests: **NOT RUN**.

The model does not authorize privileged actions. It does not change the remote
architecture in `REMOTE_PORT.md`. Remote Holder persistence, SSH, and remote
sessions are outside this helper's authority.

Aquarium is a **behavior-only reference** through the description in issue #70.
Aquarium code provenance for this threat model is **NONE**: no source was copied,
translated, vendored, or used as implementation input. Its license and current
behavior were not researched here.

## Security objectives

A future design must:

1. change the global sleep state only for an explicitly consenting, exactly
   identified, eligible local execution;
2. prevent an unprivileged or stale client from acquiring root power authority,
   directly or through a genuine Engine;
3. independently enforce bounded duration and conservative safety policy at the
   privileged boundary;
4. preserve and restore state without clobbering another utility's state;
5. recover deterministically across Engine, app, helper, update, uninstall, and
   machine failures;
6. tell the truth: readiness is shown only while identity, safety, lease, mutation,
   and readback evidence are current; and
7. preserve ordinary Zeus operation when the optional feature is unavailable.

These objectives are proposed acceptance conditions, not verified properties.

## Assets

| Asset | Required protection |
|---|---|
| Normal macOS lid-sleep behavior | No unauthorized, unbounded, invisible, or stranded disablement |
| User and hardware safety | Release on AC loss, serious/critical thermal pressure, unknown evidence, override, or expiry; never suppress emergency protections |
| Root authority | No generic root execution, path, environment, command, or value surface |
| Explicit user intent | Per-execution consent cannot be forged, replayed, inherited, or silently re-armed |
| Session execution identity | PID reuse, respawn, adoption, hibernation, and stale records cannot transfer consent |
| Lease and ownership facts | Durable, bounded, authentic, and tied to Engine/helper/boot incarnations |
| Recovery journal | Root-owned, durable, schema-versioned, bounded, and safe against links/type substitution |
| App/Engine/helper identity | Exact signed executable, team/designated requirement, Build ID, protocol, capability, UID, and freshness |
| Safety signals | Authentic enough for policy, fresh, internally consistent, and fail-closed when unavailable |
| Release lifecycle | Install, update, rollback, duplicate copies, and removal cannot bypass identity or strand state |
| User-facing truth | No false “Safe to close,” false restoration, or hidden active state |
| Sensitive data | No prompts, terminal content, credentials, full environment, Agent command line, or IPC payload logging |
| Ordinary and remote Zeus behavior | Optional feature failure must not break sessions or alter remote transport |

## Threat actors and failure agents

| Actor/agent | Capability considered |
|---|---|
| Same-UID malicious local program | Connects to user sockets, calls public Engine actions, races files/processes, replays captured messages |
| Different local user | Attempts helper access during fast user switching or through shared resources |
| Root/admin malware | Out of prevention scope: it can already change system power state; detection and honest status are still desirable |
| Modified or unsigned Zeus binary | Claims Zeus identifiers or attempts direct/proxied helper access |
| Old correctly signed Zeus binary | Has the same Team ID but vulnerable or incompatible protocol/policy |
| Malicious/malformed IPC peer | Sends stale, replayed, oversized, truncated, reordered, or high-rate frames |
| External sleep-management utility | Writes the same system-wide boolean before, during, or after Zeus operations |
| Filesystem attacker | Uses user-writable bundles/state, symlinks, hard links, type changes, path substitution, or disk exhaustion |
| Update/removal race | Leaves old/new app, Engine, or helper alive across partial replacement or rollback |
| Component crash/hang | App, Engine, helper, fixed command, launchd registration, or machine stops at any state transition |
| Environmental failure | AC removal, thermal pressure, unreadable sensors, suspend/wake, reboot, clock change, or OS behavior change |
| Misuse by an authorized user | Puts an active Mac in a bag, misunderstands a global effect, or loses the visible override |

## Components and trust boundaries

```text
User
  | TB1: consent, truthful state, override
Desktop app (unprivileged)
  | TB2: app -> owner-local Engine action authorization
Local Rust Engine (session and eligibility authority)
  | TB3: authenticated/versioned IPC across user -> root boundary
Optional zeus-power-helper (root; lease, safety, journal, recovery only)
  | TB4: fixed operation -> macOS global power state
  | TB5: readback and OS safety notifications -> helper policy

Signed app/helper bundle and updater -- TB6 --> installed helper identity
Root-owned recovery directory ------ TB7 --> helper recovery state
External sleep utilities -------- shared global boolean -------- TB4
```

### TB1 — user to desktop

The desktop requests consent for named live local executions and shows effective
state. Installation consent is not session consent. A reachable **Allow Sleep
Now** action must revoke grants and verify restoration. If the GUI disappears,
the design must either retain a truthful indication/override elsewhere or release
before exit.

### TB2 — desktop to Engine

The current Engine is authoritative for sessions. An owner-only local channel
alone does not prove that the human used the genuine desktop: arbitrary same-UID
software could ask a genuine Engine to proxy a power request. The future typed
action needs explicit caller authorization, anti-replay, intent freshness, and a
defined GUI/CLI policy. MCP, hooks, Agents, terminal contents, and session child
processes must have no route to enable or renew the feature.

### TB3 — Engine to privileged helper

Authentication must be mutual and connection-bound. Candidate evidence includes
an unforgeable audit token, exact designated requirement, expected Team ID and
executable identifier, authorized UID, exact Build ID/protocol/capabilities,
Engine incarnation, helper boot nonce, monotonic request sequence, and a bounded
lease identifier. A request's claimed PID, UID, path, bundle ID, or version is not
authentication. Team ID alone does not reject an old correctly signed binary.

### TB4 — helper to global power state

The helper may expose only fixed `status`, `acquire`, `renew`, `release`, and
`prepare_uninstall` semantics. It must accept no executable, argv, path,
environment, shell text, or `pmset` value. If a command is required, use an
internally fixed absolute executable and argv, a bounded runtime, and state
readback. Command success alone is not proof of activation or restoration.

This boundary is intrinsically shared with other utilities because
`SleepDisabled` is a global boolean without an ownership token.

### TB5 — operating-system evidence

Power source, battery, lid, thermal, boot, and time evidence cross into the
policy decision. Unknown, stale, unsupported, or inconsistent evidence blocks
acquisition and revokes an active grant. Serious/critical thermal pressure and
AC removal restore normal sleep. Emergency sleep and other macOS protection must
remain outside Zeus control.

### TB6 — packaging and code identity

The privileged boundary must authenticate the actual installed executable, not
a path in a user-writable staged bundle. The design must prevent substitution
between verification and execution. Old-app/new-helper, new-app/old-helper,
duplicate bundle, relocation, rollback, and downgrade cases fail unavailable.
The existing shell app swap in `crates/zeus-updater/src/install.rs` is not a root
helper lifecycle and must never become one.

### TB7 — durable recovery state

Directories must be root-owned `0700`; regular state files must be root-owned
`0600`. Validate owner, type, link state, schema, bounds, boot identity, and every
component before use. Use atomic durable writes. Acquisition journals observed
prior state and intent before mutation, then records ownership only after
readback. Ambiguous or corrupt state blocks new acquisition; it is not permission
to force a default.

## Session authorization model to test

The Engine would aggregate exact, explicitly selected **local held execution
generations** into one helper lease. A proposed identity tuple is:

```text
Session ID
+ local execution location
+ Holder incarnation
+ child PID and process start identity
+ hibernation/liveness facts
+ Engine incarnation
+ original consent deadline
```

The complete tuple is not present in repository models. `SessionRecord` in
`crates/zeus-proto/src/model.rs` has status and host/hibernation fields but no
power consent or local execution generation. `HolderStat` in
`crates/zeus-engine/src/holder/protocol.rs` has PID/liveness, optional output
epoch, and optional Holder incarnation and child start identity. Older Holders
can omit those identity fields. `crates/zeus-engine/src/registry.rs` can adopt a
live Holder, respawn under an existing record, and stop/continue a hibernated
tree. Thus eligibility must fail closed when exact identity is absent, and it
must also bind consent plus Engine/helper incarnations.

The policy hypothesis is: exit, hibernation, explicit revoke, expiry, migration,
respawn, identity mismatch, failed adoption, safety cutoff, or **Allow Sleep Now**
removes eligibility. Attach, wake, queued input, restart, or adoption does not
silently re-arm consent. `Working` is not the sole eligibility signal because a
long child command can outlive screen-derived status.

## Abuse cases, proposed controls, and gates

All controls are unimplemented proposals unless a repository fact is cited.

| ID | Abuse/failure case | Proposed preventive/detective controls | Required gate |
|---|---|---|---|
| A1 | Same-UID malware asks the real Engine to enable power mode | Typed privileged action; authorize genuine desktop/audited caller; explicit short-lived human intent; deny Agents, hooks, MCP, and generic CLI by default; rate-limit | Adversarial two-hop test; mock first, signed lab later |
| A2 | Client connects directly to root helper | Audit-token identity, designated requirement, UID, Build/protocol floor, mutual challenge, nonce and sequence | Unsigned/wrong-team/wrong-binary/wrong-UID tests |
| A3 | Old correctly signed app or Engine is accepted | Exact build allow policy and anti-downgrade floor bound to installed helper; incompatible removal-only recovery path | Old/new version-pair matrix |
| A4 | Captured request renews or revives a lease | Connection-bound helper boot nonce, Engine incarnation, monotonically increasing sequence, lease ID, monotonic deadline; no reboot revival | Replay/reorder/stale/reboot tests |
| A5 | Malformed peer causes root crash or resource exhaustion | Length-prefixed bounded codec; connection/frame/state limits; timeouts; rate limits; fuzzing; redacted reason codes | Fuzz, oversized, truncated, flood tests |
| A6 | Helper becomes a generic root command service | Closed method enum; no caller paths/argv/env/values; fixed absolute operations; no shell; bounded child; readback | API/source review plus negative IPC tests |
| A7 | User-writable or linked state is trusted by root | Root-owned directory/files; `openat`-style no-follow/type/owner checks; size/schema limits; atomic durable replace | Permission, symlink, hard-link, type-swap, disk-full tests |
| A8 | PID reuse or session respawn inherits consent | Start identity plus Holder/execution incarnation; consent bound to original deadline; fail closed for older unverifiable holders | PID reuse, respawn, adoption, hibernate/wake fixtures |
| A9 | UI says ready before activation or after evidence stales | Effective-state snapshot includes helper identity, lease, session tuple, safety freshness, mutation readback; UI expires evidence locally | Fake-clock UI truth tests and fault injection |
| A10 | GUI exit hides active global policy | Persistent truthful indicator and override, or verified release-before-exit; Engine contract distinguishes quit from crash | App quit/crash and Engine-survival tests |
| A11 | External utility already controls or races the boolean | Refuse when already disabled/ownership uncertain; observe unexpected changes; journal prior/owned state; surface conflict; never blindly force `0` | Concurrent writer matrix; unresolved races require no-go |
| A12 | Helper dies with sleep still disabled | Supervised restart/recovery proof, durable crash-point state, launchd throttle/disable tests, verified bounded restoration | **Mandatory no-go blocker; see below** |
| A13 | Engine dies while helper lives | Helper-enforced monotonic TTL and independent safety checks; restoration/readback on expiry | Fault-injected Engine death against measured bound |
| A14 | Update/rollback leaves mixed trusted versions | Quiesce acquisition; restore/readback; pin consumed identity; exact compatibility floor; fresh handshake; removal works when incompatible | Signed upgrade/rollback/interruption matrix |
| A15 | Uninstall/app deletion strands global state | Prepare-uninstall releases and verifies before unregister; retain recovery facts on failure; reviewed manual procedure | UI, app deletion, Homebrew, partial removal tests |
| A16 | Safety source is missing, stale, spoof-like, or contradictory | Privileged-side verified source, freshness budget, event-loss detection, fail closed; no user IPC safety override | Unknown/stale/rapid transition tests |
| A17 | AC is removed or thermal pressure rises | Helper independently releases; verifies; clears grants; requires explicit re-arm | Measured attended physical-lab trials |
| A18 | User puts active Mac in a bag | Feature off by default; AC-only v1; bounded duration; persistent warning/indicator; “ventilated desk, not a bag”; emergency protection unchanged | UX review; residual misuse risk explicitly accepted or no-go |
| A19 | Sensitive Agent data reaches privileged logs | Helper never enumerates Agent processes or reads prompts, terminals, environments, credentials, or command lines; structured minimal logs | Log/content inspection and injected-secret test |
| A20 | Two users/Engines contend | One explicitly authorized UID and one Engine incarnation/lease; reject fast-user-switch and second-Engine cases until policy exists | Multi-user/multi-Engine signed lab test |
| A21 | Wall clock or reboot extends/revives consent | Monotonic expiry, boot identity, no lease revival, fresh consent after restart for proposed v1 | Fake clock plus reboot lab test |
| A22 | OS changes undocumented behavior | Exact OS support matrix; activation and restoration readback; fail unavailable after update until validated; conditional UI wording | OS-update physical regression gate |

## State-machine and crash analysis

Candidate privileged state machine:

```text
Unavailable / Disabled
  -> Authenticating
  -> CheckingSafety
  -> Preparing (intent journal durable)
  -> Mutating
  -> Armed (mutation read back, ownership committed, lease live)
  -> Releasing
  -> Disabled (restoration read back, journal durably removed)

Any ambiguity -> RecoveryRequired (no acquisition and no readiness claim)
Startup -> RecoverBeforeAcquire -> Disabled or RecoveryRequired
```

Fault injection must stop the process before and after each journal write, sync,
rename, mutation, readback, commit, restoration, and journal deletion. Expected
outcomes must be defined for missing/corrupt/unsupported records, disk full,
command hang/failure, reboot, and changed boot identity. “Try to set 0” is not a
deterministic recovery policy when ownership is ambiguous.

## Mandatory no-go blocker: helper death and a global boolean

A renewable lease is only enforceable while some privileged process is alive to
observe expiry and restore state. `SleepDisabled` does not expire on its own and
has no Zeus ownership token. If the helper dies after setting it, the global bit
can outlive the component responsible for clearing it. A journal describes intent
but cannot execute recovery. A concurrent utility can also write the same value,
so read-before/write/read-after cannot establish race-free ownership.

This combination is an explicit **NO-GO BLOCKER**. Shipment is forbidden unless
reviewed evidence demonstrates:

- bounded verified restoration after helper kill, crash loop, launchd throttling,
  registration disablement, corrupt/missing state, interrupted update/removal,
  reboot, and every journal crash point; and
- a coexistence rule that does not silently clobber externally controlled state,
  with every remaining race explicitly accepted by security/product review.

The initial `<= 60 s` helper recovery target in `CLOSED_LID_DECISIONS.md` is only
a **PROPOSAL REQUIRING MEASUREMENT**. The current design cannot guarantee it.
Manual recovery, a warning, or an unmeasured launchd restart is not a substitute.
Do not add a second privileged watchdog without a new architecture review. If
the blocker cannot be closed, the required P6 result is **NO-GO**.

## Proposed safety and performance budgets

This threat model adopts the proposal table in `CLOSED_LID_DECISIONS.md`: 15 s
renewal, 45 s TTL, 8 h maximum consent, 5 s safety freshness, 2 s operation
timeout, `<= 5 s` AC/thermal restoration, `<= 60 s` Engine/helper-death
restoration targets, zero settled inactive recurring wakeups, `<= 4` active
renewals/minute, `<= 0.1%` inactive CPU, `<= 0.5%` active control-plane CPU, and
`<= 20 MiB` helper resident memory.

Every number is a **PROPOSAL REQUIRING MEASUREMENT**, not evidence. Security tests
must measure worst case as well as typical behavior. A performance target cannot
weaken authentication, PTY/session identity, safety, or restoration.

## Evidence matrix

| Claim/gate | Current evidence | Status | Required evidence |
|---|---|---|---|
| Threat inventory matches requested risks | Issue #70; `CLOSED_LID_PLAN.md` | DOCUMENT REVIEW ONLY | Security review and abuse-case signoff |
| Privilege exception is allowed | None; conflicts with `../AGENTS.md` | **NOT APPROVED** | Written narrow local-only exception |
| App/Engine/helper path resists unauthorized proxying | Proposed controls only | **NOT TESTED** | Same-UID, second-user, wrong identity, old signed build, replay tests |
| Helper IPC is bounded and non-generic | Proposed interface only | **NOT IMPLEMENTED** | Codec/API review, fuzzing, negative command/value/path tests |
| Exact execution authorization is possible | Repo exposes partial identity facts | DESIGN GAP | Pure reducer plus lifecycle/identity fixtures |
| Journal recovery is deterministic | Proposed state machine only | **NOT TESTED** | All crash points, permissions, links, corruption, disk full |
| Global-boolean coexistence is safe | Issue and plan identify non-tokenized state | **UNRESOLVED NO-GO BLOCKER** | Concurrent physical writers and approved residual-risk decision |
| Helper-death cleanup is bounded | No self-enforcing lease mechanism | **UNRESOLVED NO-GO BLOCKER** | Signed lab kill/crash-loop/throttle/disable/recovery results |
| AC/thermal/unknown evidence fails safely | Proposed policy only | **NOT TESTED** | Signal injection and attended physical validation |
| `pmset` behavior works on supported Macs | No execution in this work | **PHYSICAL TEST NOT RUN** | macOS 15+ Apple silicon and Intel records |
| ServiceManagement identity/lifecycle works | No execution in this work | **PHYSICAL TEST NOT RUN** | Signed install/update/rollback/uninstall lab report |
| Closed-lid Agent makes continuous progress | No execution in this work | **PHYSICAL TEST NOT RUN** | Same PID and start identity plus timestamped progress |
| Restoration returns ordinary lid sleep | No execution in this work | **PHYSICAL TEST NOT RUN** | Normal release, failures, reboot, removal |
| Numeric budgets are satisfied | Proposal table only | **NOT MEASURED** | Recorded workloads, raw data, percentiles, worst cases |
| Aquarium code is a valid provenance source | No Aquarium source/license research | NOT ESTABLISHED; BEHAVIOR-ONLY | Independent authorship or separate license approval |

## Mock versus physical lab

### Mock/unprivileged phase

Allowed without an architecture exception:

- eligibility and consent reducer with fake execution identities and clocks;
- both IPC-hop authorization model using a backend that cannot call `pmset`;
- bounded-frame parser fuzzing and replay/nonce/sequence tests;
- journal fixtures in an unprivileged temporary directory with simulated crash
  points, permissions, links, corruption, and disk errors;
- simulated AC, thermal, lid, time, restart, update, and external-writer events;
- UI state/copy tests that identify the backend as fake.

Mock results can reject a design. They cannot validate ServiceManagement,
code-signing audit tokens at the privileged boundary, real power behavior,
launchd recovery bounds, energy use, ordinary lid sleep restoration, or the
user-facing closed-lid promise.

### Signed physical-lab phase

This phase requires a separate written approval and a dedicated, attended,
ventilated Mac with recovery access, test signing identity, isolated registration
and journal namespace, known initial state, and verified cleanup. It includes:

- ServiceManagement registration and exact peer identity;
- real `pmset` mutation/readback and concurrent external writers;
- lid-close progress with PID plus process start identity;
- AC removal, available thermal tests, suspend/wake, reboot, and OS update;
- Engine/helper kill, crash loops, launchd throttle/disable, update, rollback,
  uninstall, app deletion, and manual recovery; and
- inactive/active CPU, memory, energy, wakeup, renewal, and reaction measurements.

No physical `pmset` or ServiceManagement work was performed here: **NOT RUN**.
No default CI test may register a service, request privilege, or mutate the host's
power state.

## Residual risks that require explicit acceptance

Even after successful tests, reviewers must decide whether these remain suitable
for shipment:

- dependence on an undocumented mechanism that macOS can change or remove;
- unavoidable ambiguity/races created by a shared non-tokenized boolean;
- a period of continued global wakefulness between a failure and restoration;
- authorized misuse in an enclosed space;
- limits of thermal, power-source, and lid evidence;
- old or duplicate signed app copies and recovery-only compatibility;
- the mismatch between a conditional mechanism and user expectations created by
  “Safe to close”; and
- manual recovery steps that may themselves require informed authorization.

Acceptance must name an owner, platform matrix, measured bounds, user wording,
and stop conditions. Silence is rejection, not acceptance.

## Current disposition

The design remains at documentation/mock stage. Architecture exception and
shipment go/no-go are **NOT APPROVED**. The helper-death/global-boolean problem is
an explicit unresolved **NO-GO BLOCKER**. Physical `pmset` and ServiceManagement
tests are **NOT RUN**.
