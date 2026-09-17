# Closed-lid production milestone 1 — inert lifecycle and package contract

## Status

**IMPLEMENTED FOR REVIEW; NOT PRODUCTION READY.**

This is the first production-targeted milestone allowed by the issue #70 exception
in `../AGENTS.md`. It intentionally cannot register a service, request
authorization, listen for privileged requests, invoke a power tool, or change host
power state. The packaged Helper scaffold reports `power_helper_unavailable` and
exits with status 78.

No release gate is closed by this milestone. In particular, the shared global
power boolean and bounded recovery after Helper death remain explicit P6 no-go
blockers. No physical or ServiceManagement lifecycle test has run.

## Repository and design check

The implementation followed these existing boundaries:

- `REMOTE_PORT.md` remains unchanged. Remote SSH, Remote Holders, persistence,
  packaging, and capabilities are not used by the local power feature.
- The Rust Engine remains the future session/consent authority. The minimal
  Helper boundary owns only lease, safety, journal, recovery, and fixed power
  operations. This milestone implements only portable policy and inert package
  contracts.
- Existing local Holder and child birth tokens are the exact-execution base.
  Session ID, PID, display status, or adoption alone never grants consent.
- Ordinary builds and CI use policy, filesystem fixtures, and an inert executable.
  They do not register, authorize, or mutate power.
- The existing updater is not treated as a privileged transaction and was not
  extended into one.

Independent security and lifecycle reviewers separately reviewed the required
policy documents and the branch. Their critical findings drove the consent fence,
exclusive lifetime lock, journal incarnation fields, immutable consent horizon,
and uninstall quiescence changes below.

## Implemented contract

### Explicit consent and lease coordination

`zeus-power::consent` now issues in-memory grants for one atomic selection. Each
grant binds:

- a fresh Engine incarnation;
- a strictly increasing authenticated action generation;
- a nonzero consent nonce;
- one exact local execution generation and process birth identities; and
- one immutable monotonic deadline.

Reconciliation can remove grants but cannot add them. Identity substitution,
respawn/PID reuse, expiry, or clock regression removes authority. Revocation
consumes its own generation fence, so delayed authorize work at or below the
fence cannot undo **Allow Sleep Now**. A new Engine starts with no grants.

The lease reducer accepts these opaque grants rather than bare eligibility facts.
All members of one active selection must have the same Engine incarnation,
consent generation, nonce, and deadline. Safety trips remain sticky until a new
explicit generation is armed.

### Versioned Engine/Helper protocol

Power protocol 1.1 adds consent generation, consent nonce, and an immutable
first-acquire maximum duration. Renew cannot create a new consent horizon.
Acquire/Renew reject zero lease or consent identities, empty or duplicate
execution sets, mixed boot identities, invalid TTLs, oversized sets, and invalid
total duration. Prepare-uninstall carries a nonzero transaction identity.

The protocol remains a bounded model. No transport claims are made. Mutual audit
token/code-signature authentication, Hello negotiation, controller replacement,
and the genuine desktop-to-Engine human-intent hop remain future reviewed work.

### Exclusive lifetime and durable journal

Before journal access, `LockedStateDirectory` opens one supplied fixed state
directory with no-follow and validates owner, type, and exact `0700` mode. It then
opens a `0600`, single-link lock file with `CLOEXEC` and takes a nonblocking
exclusive kernel lock. A second live Helper cannot read or recover the first
Helper's journal. Tests cover a separate process and prove an exec child does not
inherit the lock.

The checksummed, bounded journal binds stable installation owner, host boot,
writer Helper instance, Engine incarnation, lease ID, consent generation,
immutable hard deadline, mutation generation, prior observation, and recovery
phase. Writes use a same-directory exclusive temp file, full write, file sync,
atomic rename, and parent-directory sync. Removal validates the record, unlinks,
and syncs the directory. Wrong owner/type/mode/link, symlink, corruption,
unsupported state, and mismatched writer identity fail closed.

The pure recovery machine now enters `RemovalPrepared` after verified cleanup and
rejects later acquisition. A changed boot is ambiguous, not permission to restore.
These properties are necessary but do not solve ownership of an external
non-tokenized system setting.

### macOS package scaffold

`zeus-power-helper` is a separate minimal Rust crate. Its executable is built only
with the explicit `macos-service-scaffold` feature and refuses non-macOS targets.
It has no listener or operational backend and always reports unavailable.

The app package builds universal arm64/x86_64 slices, places the executable only
at:

```text
zeus.app/Contents/Library/HelperTools/com.zeus.zeus.power-helper
```

and places fixed demand-only launchd metadata at:

```text
zeus.app/Contents/Library/LaunchDaemons/com.zeus.zeus.power-helper.plist
```

The Helper is signed independently with hardened runtime before the outer app is
signed. Metadata tests reject command, environment, socket, schedule, path-watch,
and caller-selected authority surfaces. There is no registration API call,
authorization request, `launchctl`, `sudo`, shell installer, or power operation.
An ad-hoc package remains explicitly unavailable.

## Automated evidence

Safe checks for this milestone include:

```sh
cargo test -p zeus-power -p zeus-power-helper
cargo build -p zeus-power-helper --bin zeus-power-helper --features macos-service-scaffold
cargo clippy -p zeus-power -p zeus-power-helper --all-targets -- -D warnings
bash -n scripts/package.sh
bash scripts/check-power-helper-package.sh --self-test
```

Coverage includes exact consent generation/fencing, PID reuse, Engine restart,
expiry and clock regression, safety latching, protocol bounds, lock contention
across processes, close-on-exec behavior, journal permissions/symlinks/checksum,
atomic replacement semantics, recovery crash points, and fixed launchd metadata.
These are deterministic local tests, not signed lifecycle or physical evidence.

## Open blockers and next gates

The following remain mandatory before any real backend or readiness claim:

1. authenticate the genuine desktop-to-Engine user action so same-UID software,
   CLI, MCP, hooks, Agents, and terminal content cannot proxy consent;
2. implement and adversarially test mutual audit-token/code-signature IPC with an
   explicit bounded Hello, exact Build IDs, protocol/capability negotiation,
   authorized UID, controller replacement, replay protection, and response type
   binding;
3. connect exact Engine Holder identity, host boot, and execution generation
   without allowing deferred/adopted/woken/spoofed sockets to inherit consent;
4. add independent privileged-boundary AC, battery, lid, thermal, boot,
   monotonic-clock, emergency, and Helper-health evidence;
5. finish update, rollback, quiesce, removal, duplicate-app, old/new pair, and
   recovery-only compatibility transactions;
6. resolve and obtain owner approval for coexistence with the non-tokenized
   system-wide setting; ambiguous ownership must remain blocked;
7. prove bounded restoration for Engine exit, Helper kill/restart/crash loop,
   registration disablement, update/removal interruption, reboot, corrupt or
   missing journal, and operation timeout/failure;
8. run separately approved signed/notarized registration and lifecycle tests on
   dedicated Macs; and
9. run the full attended physical architecture/OS matrix plus latency, energy,
   wakeup, inactive CPU, and restoration measurements.

Until all recorded gates pass, Zeus must show the feature as unavailable or
experimental and must not display an unqualified **Safe to close** claim.
