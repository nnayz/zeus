# Closed-lid spike evidence — #70

## Scope of this record

This file records reproducible, read-only evidence collected while executing
`CLOSED_LID_PLAN.md`. It is not a supported-platform certification and is not
approval to ship a privileged helper. No service was installed, no root command
was run, no power setting was changed, and the lid was not closed during these
checks.

Collected at `2026-09-15T15:40:21Z` on one Apple-silicon development Mac:

| Item | Observed value |
|---|---|
| macOS | 27.0 (26A428) |
| Architecture | arm64 |
| Command Line Tools macOS SDK | 26.5 |
| `notarytool` | 1.1.2 (41) |

This one host is outside the macOS 15 physical acceptance matrix requested by
#70. It provides compile-time/API evidence only. Intel and every supported
release remain untested.

## Primary local SDK evidence

The installed SDK's
`System/Library/Frameworks/IOKit.framework/Headers/pwr_mgt/IOPMLib.h`
documents `kIOPMAssertPreventUserIdleSystemSleep` as preventing idle sleep but
states that the system may still sleep for lid close, Apple-menu sleep, low
battery, or other reasons. This confirms that an ordinary idle assertion cannot
support Zeus's proposed closed-lid promise.

Reproduce:

```sh
SDK="$(xcrun --sdk macosx --show-sdk-path)"
sed -n '275,292p'   "$SDK/System/Library/Frameworks/IOKit.framework/Headers/pwr_mgt/IOPMLib.h"
```

The installed `pmset(1)` manual says modifications require root. Its documented
settings do not list `disablesleep`, and `pmset -g cap` on this host does not
advertise it. Therefore this spike must continue to classify `disablesleep` as
undocumented. Observable behavior or successful notarization cannot upgrade it
to a supported API.

Reproduce without mutation:

```sh
MANPAGER=cat man pmset | col -bx
pmset -g cap
pmset -g custom
```

The installed
`System/Library/Frameworks/ServiceManagement.framework/Headers/SMAppService.h`
states that `SMAppService` controls helpers embedded in a signed app; it replaces
LaunchDaemon installation through `/Library/LaunchDaemons`; apps containing
LaunchDaemons must be notarized; and changed daemon executables or plists must be
re-registered (with unregister/re-register recommended when the executable
changes). The API is available from macOS 13, so it exists across Zeus's macOS
15+ deployment target. This establishes an API candidate, not proof that the
planned Rust registration, authorization, update, or recovery design works.

Reproduce:

```sh
SDK="$(xcrun --sdk macosx --show-sdk-path)"
sed -n '39,70p'   "$SDK/System/Library/Frameworks/ServiceManagement.framework/Headers/SMAppService.h"
```

## Evidence still required

- Maintainer approval of the narrow local-only architecture exception.
- Legal/provenance review. No Aquarium code was read or used for this spike.
- A separately signed, mock-only ServiceManagement registration and authenticated
  audit-token IPC lab on a dedicated Mac.
- Evidence that a root LaunchDaemon registered through the chosen supported flow
  has the required lifecycle and identity behavior on each supported OS.
- Attended physical lid-close tests for macOS 15+ on Apple silicon and Intel.
- `disablesleep` acquire/readback/release and each failure/recovery point on those
  machines, only after explicit lab authorization and a reviewed recovery path.
- Notarization/distribution policy review for relying on undocumented behavior.
- Measured CPU, memory, energy, wakeups, reaction times, and session progress.
- Conflict testing with another writer of the global setting and a go/no-go
  decision about the unavoidable lack of ownership tokens.
- Helper crash-loop/disable/removal tests that prove restoration or force no-go.

Until those rows are closed, the only supported claims are ordinary lid-open
idle-sleep prevention and continued execution on an eligible remote host.
