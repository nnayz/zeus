# Closed-lid release packaging spike — issue #70

## Status and boundary

This is release-spike and inert package-scaffold evidence, not approval to ship
an operational privileged helper. The first production-target milestone now
packages an independently signed macOS-only executable and fixed demand-only
launchd metadata, but it does not register a service, request authorization,
listen for IPC, install a daemon, or change power state. The executable always
reports `power_helper_unavailable`. `CLOSED_LID_PLAN.md` remains controlling and
all stop/go gates still apply.

This spike fixes one testable **future package contract** so a release artifact
cannot silently find a helper in an alternate location:

```text
zeus.app/Contents/Library/HelperTools/com.zeus.zeus.power-helper
```

The helper's signing identifier is fixed as
`com.zeus.zeus.power-helper`. The release app identifier remains
`com.zeus.zeus`. The first milestone now fixes demand-only launchd metadata at
`Contents/Library/LaunchDaemons/com.zeus.zeus.power-helper.plist` and protocol
1.1 policy models. It still does not implement a registration API, authenticated
transport, privileged entitlements, or installed runtime identity. Those require
the separate signed macOS/security lab and review in `CLOSED_LID_PLAN.md`.

## Read-only bundle check

Run:

```sh
zeus/scripts/check-power-helper-package.sh /path/to/zeus.app
zeus/scripts/check-power-helper-package.sh --self-test
```

The checker uses only read operations in normal bundle-validation mode. It does
not invoke `sudo`, ServiceManagement, `launchctl`, an installer, `pmset`, or any
power API. It does not search alternate bundle paths or inspect a currently
installed daemon. It checks all of these conditions and stops on the first
failure:

1. The app is a real, non-symlink directory.
2. The helper exists at the exact path above and is a regular non-symlink file.
3. Its Mach-O slices are **exactly** `arm64` and `x86_64`.
4. The helper passes `codesign --verify --strict` and the whole app passes
   `codesign --verify --deep --strict`.
5. The app and helper have their exact signing identifiers and exactly the same
   valid Team ID.
6. Signing metadata for both binaries includes the hardened-runtime flag.
7. Both have a parseable designated requirement for their distinct identifiers.
8. `codesign` evaluates each exact identifier against the same Apple-generic
   anchor and the Team ID read from the app.

The designated-requirement strings are intentionally not compared byte for
byte. Their identifiers must differ. Instead, `codesign` evaluates an app policy
and a helper policy with the same Apple signing anchor and app Team ID. This is
the relevant same-publisher match while keeping distinct executable identities.
The future privileged boundary must authenticate the consumed executable again;
this package check cannot prevent a later path substitution or
verification-to-execution race.

Exit status is fail closed:

| Status | Meaning |
|---:|---|
| `0` | Every package check passed |
| `1` | Present bundle/helper is invalid |
| `2` | Usage, platform, or required Apple-tool error |
| `3` | `power_helper_unavailable`: helper is absent at the exact path |

Exit `3` remains expected for loose or older Zeus packages without the scaffold.
A packaged scaffold can pass this read-only contract, but the optional closed-lid
feature still remains unavailable because no registration or operational backend
exists. Ordinary Zeus sessions must continue to work.

`--self-test` uses throwaway files and mocked `lipo`/`codesign` results. It covers
the success path plus missing, symlink, architecture, strict-signature, Team ID,
designated-requirement, and hardened-runtime failures. The temporary fixture is
removed. The self-test proves parser and fail-closed control flow. It does **not**
prove Apple signature, notarization, ServiceManagement registration, or runtime
identity behavior.

A real positive test is necessarily a manual or credentialed release-CI gate
until an approved helper exists. The repository cannot synthesize a meaningful
Developer ID signature or notarization ticket with mocks. When a prototype is
approved, run the checker on the final signed and stapled `.app` extracted from
both the notarized ZIP and DMG, then also run Apple's notarization assessment and
the registration/lifecycle matrix below on dedicated Macs. Never weaken the
checker for ad-hoc development builds; use a mock backend instead.

## Update and Engine-replacement transaction matrix

The current updater waits for the GUI PID, moves the old app aside, copies the
staged app, restores the backup on an ordinary copy failure, and opens the
result. The Engine has a separate hash-based replacement path and can survive
the GUI long enough to overlap an app swap. Neither path currently coordinates
a privileged helper. Therefore the existing app swap is not a power-helper
transaction and must not be treated as one.

Every future row starts with the same invariant: stop new acquisitions, release
the lease, and verify owned power state is restored **before** changing the app
or registration. Failure to prove restoration blocks the helper transaction.
It must not block ordinary session execution.

| Case / interruption point | Required future transaction and result | Evidence required before release |
|---|---|---|
| Current app has no helper | Install/update the ordinary app normally. Report `power_helper_unavailable`; do not search or fall back. | Checker exits `3`; ordinary local and remote sessions still pass. |
| First helper-bearing install; authorization approved | Verify the final nested artifact, explain privilege/global effect, obtain explicit authorization, register through the supported API, authenticate installed Build ID/protocol, then permit a fresh lease. | Signed/notarized physical-Mac registration test for both CPU families. |
| First install; authorization denied, pending, or disabled | Keep Zeus installed and usable. Closed-lid mode stays unavailable with the actual registration state. Do not retry silently. | Tests for every ServiceManagement state and restart. |
| Update requested with no active lease | Quiesce acquisitions, verify no owned state remains, validate staged app/helper, swap the app, update registration through the supported flow, authenticate the new pair, then unquiesce. | Ordered event log with Build IDs; fault injection at every boundary. |
| Update requested with an active lease | Revoke and release first. If restoration cannot be proved, leave the old app/registration untouched and report recovery required. | Forced restoration failure shows no bundle or registration mutation. |
| GUI exits for updater while old Engine survives | GUI-PID exit is insufficient. The old Engine must acknowledge quiescence and lose renewal authority before swap. A timeout aborts before mutation. | Live old-Engine test proves it cannot renew during or after swap. |
| Bundled Engine hash changes | Revoke the old Engine's lease and authority before daemon replacement. Authenticate the new Engine/helper pair and require a new user-approved acquisition; never transfer a live lease by PID or Session ID. | Old/new Engine overlap test with monotonic identities and no renewal gap ambiguity. |
| Old Engine with new helper | Reject incompatible Build ID, protocol, capabilities, or build floor. Retain only a narrowly designed cleanup/removal operation if security review approves it. | Version-pair tests show unavailable, not downgrade/fallback. |
| New Engine with old helper | Same fail-closed behavior. The new Engine must not relax its minimum helper build merely to preserve availability. | Minimum-build and rollback fixtures. |
| Staged helper has bad path, type, slices, signature, Team ID, DR, or runtime flag | Reject before target mutation. Re-authenticate the actual installed/consumed helper later as a separate check. | This script plus path-substitution and TOCTOU tests at the privileged boundary. |
| App swap stops before old bundle is renamed | Old app and registration remain authoritative; acquisitions stay quiesced until state is reconciled. | Kill/fault injection before rename. |
| Interruption after old bundle rename but before new copy completes | Do not claim an atomic update. Recover the old app from backup, or surface an actionable incomplete state. No helper lease may survive the gap. | Power-loss simulation at each filesystem operation; verified backup recovery. |
| New app copied, registration not changed | Treat app/helper pair as unverified and unavailable. On restart, reconcile exact installed identity before acquisition. | Crash immediately after copy, before registration. |
| Registration update fails after app copy | Roll back to a coherent compatible app/helper/registration set, or leave power mode unavailable with recovery evidence. Never keep a mixed pair active. | Failure for pending approval, disabled service, API error, and helper launch failure. |
| Rollback to an older app with a newer helper | Enforce the helper/client build floor. An older signed app gets no control authority. Safe unregister/removal must not require normal control compatibility. | Downgrade tests across every supported version pair. |
| Duplicate app copies or app relocation | Do not let pathname or same Team ID select the controller. Pin the authorized executable identity/build and prefer unavailable on ambiguity. | `/Applications`, user Applications, renamed, and duplicate-copy tests. |
| User quits during daemon replacement | Reconciliation is idempotent. Neither old nor new Engine can inherit consent, and no registration operation reports success without verifying final identity and restored state. | Kill GUI, both Engine generations, and registration operation at each step. |
| Explicit uninstall or raw app deletion | Restore and verify owned state before unregistering. If deletion bypasses preparation, recovery must identify the orphaned registration/state without blindly resetting another utility's setting. | UI uninstall, Finder deletion, and recovery rehearsal. |

Cross-cutting release rules:

- The generated unprivileged updater shell must never become a root installer.
- App-file rollback alone is not helper/registration rollback.
- No old correctly signed client may bypass a minimum Build ID or protocol floor.
- Registration success is not lease success. A fresh mutual-authentication
  handshake is required after every install, update, Engine replacement, or
  rollback.
- Transaction logs contain bounded reason codes and identities only. They must
  not contain environments, prompts, terminal data, credentials, or protocol
  payloads.

## Homebrew uninstall gaps

The current `Casks/zeus.rb` has:

- `app "zeus.app"` for installation;
- `auto_updates true`, so later app replacement can occur outside Homebrew; and
- a `zap trash:` list for user preferences, caches, and application support.

It has no privileged-service lifecycle. In particular, it does not:

- ask the Engine to quiesce acquisitions;
- release a lease and verify restoration of Zeus-owned power state;
- unregister a ServiceManagement daemon;
- verify that the installed helper is gone or inactive;
- preserve/report a root-owned recovery journal when cleanup fails; or
- distinguish `brew uninstall`, `brew uninstall --zap`, Finder deletion, and an
  in-app removal flow.

Adding a blind `launchctl`, file deletion, privileged cask script, `sudo`, or
power reset is not an acceptable fix. Homebrew removal can race Zeus's built-in
updater because the cask explicitly permits auto-updates, and `zap` currently
removes user-side evidence while intentionally deleting application support.
The future design must settle who performs supported unregister, how Homebrew
requests it, what happens when the app/Engine is absent or incompatible, and
where actionable recovery evidence remains. A cask uninstall must report partial
cleanup honestly rather than claiming success while an active or orphaned
privileged component remains.

Release approval needs rehearsals of plain Homebrew uninstall, `--zap`, upgrade,
downgrade, interrupted removal, app already deleted, Engine still running,
authorization denial, and incompatible helper. The result must be either verified
restoration and removal, or a precise fail-closed state with a reviewed manual
recovery path.

## What this spike does not prove

Passing the script is necessary package evidence only. It does not prove
notarization, stapling, ServiceManagement metadata, audit-token authentication,
Build ID/protocol negotiation, safe power ownership, crash recovery, uninstall,
or physical closed-lid behavior. Those remain explicit gates in
`CLOSED_LID_PLAN.md`.
