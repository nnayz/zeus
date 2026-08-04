# diri performance record

Historical measurements in this file are from the T16 release build on Apple
Silicon, macOS 26.5.2, on 2026-07-23. They predate subsequent UI/font changes
and are context, not proof that the current release passes. Release acceptance
now comes from the packaged-artifact gate described below.

The current optimized universal bundle was measured on Apple Silicon on
2026-07-29 with the deterministic stress fixture:

| Packaged 0.2.0 scenario | Physical footprint | Mean idle CPU | Peak idle CPU |
| --- | ---: | ---: | ---: |
| Normal 1100×700 window | 62.4 MB | 0.533% | 0.600% |
| Large 1800×1100 window | 102.4 MB | 0.317% | 0.400% |

These are physical-footprint measurements, not Activity Monitor's larger
virtual-memory figure. A follow-up 10-second stack sample found the main thread
blocked in AppKit for 7,600 of 7,698 samples and only 15 GPUI window steps, with
no app-owned periodic render task.

```sh
export PATH=/tmp/diri-cargo-home/bin:$PATH
export CARGO_HOME=/tmp/diri-cargo-home
export RUSTUP_HOME=/tmp/diri-rustup-home
export CARGO_TARGET_DIR=/tmp/diri-shared-target
cargo build -p diri-app --release
```

The shared target is measurement/build cache only. It must never be packaged or
shipped.

## Memory

`DIRI_PERF_LARGE_WINDOW=1` is a retained profiling switch that starts the app at
1800×1100 instead of 1100×700. Samples use `/usr/bin/vmmap -summary <pid>` after
startup work and geometry have settled.

### Before and after

| Release-build sample | Physical footprint | IOSurface resident | owned unmapped (graphics) resident | MALLOC_LARGE resident | MALLOC_SMALL resident |
| --- | ---: | ---: | ---: | ---: | ---: |
| Handoff baseline (prior run) | ~429 MB | ~125 MB | not recorded | ~111 MB | ~52 MB |
| Reproduced large-window baseline, before geometry quiescence | 411.6 MB | 92.5 MB | 242.6 MB | 4.0 MB | 41.7 MB |
| Large window, after fix and settle | 202.3 MB | 92.5 MB | 50.6 MB | 4.0 MB | 39.2 MB |
| Normal window, after fix and settle | 109.2 MB | 36.3 MB | 20.5 MB | absent | 37.4 MB |
| Normal window after 100 normal↔large resize cycles | 110.6 MB | 36.3 MB | 0.1 MB resident / 20.4 MB swapped | absent | 25.2 MB |
| Normal window after 40 min / 27 periodic large→normal transitions | 116.3 MB | 0 MB resident / 36.3 MB swapped | 0.1 MB resident / 20.4 MB swapped | absent | 23.5 MB |

The reproduced baseline was captured after merging current `main`, so it
includes the earlier system-font fix. That fix removed the original persistent
~111 MB `MALLOC_LARGE` font copy. The remaining large-window regression was GPU
retirement, not a CPU heap leak.

The red/green probe was a three-second `SIGSTOP` of only the locally built test
process. Physical footprint fell from 411.6 MB to 223.8 MB and graphics
residency from 242.6 MB to 90.6 MB without changing model state. Resuming did
not recreate the retired burst (204.8 MB). This isolated continuous decorative
frames as the condition preventing Metal from retiring resize-era resources.

That historical fix paused decorative repainting after geometry settled. The
current implementation goes further: status glyphs and the terminal cursor are
static, so they never create autonomous frame tasks. Real status changes and
terminal grid damage still repaint immediately. The window and root surface
are opaque, avoiding a persistent WindowServer backdrop/blur composition pass.

### CPU allocation retention

- Store events are applied immediately, but `StoreSnapshot` cloning and
  UI/menu publication are coalesced to one update per 16 ms display interval.
  The watch channel retains only the latest snapshot.
- Startup previously launched independent recurring usage scans over 4,729
  transcript files (about 2.4 GB on this machine). Usage now scans once at
  startup and refreshes only after store-change events, debounced by two
  seconds. Its persisted transcript ledger resumes from validated append
  offsets, handles truncation/replacement, and does no timer-driven idle work.
- Each resident terminal's decoded scrollback cache is capped at 512 rows near
  the current viewport. Evicted rows remain daemon-owned and are fetched again
  if revisited. Three resident terminals therefore cannot retain an entire
  repeatedly traversed history indefinitely.
- Every incoming terminal diff still updates its authoritative buffer, but
  selected-session paints are paced to one every 50 ms and background residents
  never invalidate the window. An active find arms only one output-rescan timer.
- The daemon source uses the same 20 fps grid ceiling and skips screen-to-cell
  extraction entirely while no output sink is attached. A later attachment
  receives a fresh full grid. These daemon changes take effect only after a
  future daemon restart; this optimization work never signaled or replaced the
  daemon that was already running.

### Acceptance

- Historical idle target, under 250 MB: **pass**, 109.2 MB at normal size and 202.3 MB at
  the 1800×1100 stress size.
- Historical resize-churn target, under 300 MB: **pass**, 110.6 MB after 100 alternating
  geometry transitions.
- Historical long-use target, under 300 MB: **pass**, 116.3 MB after 40 minutes. The same
  process ran 27 minute-spaced large→normal transitions and the normal
  90-second usage refresh loop. Its physical peak remained launch-bound at
  318.4 MB; the footprint at the long-use checkpoint was 183.7 MB below budget.

## Packaged-release gate

`scripts/perf-gate.sh` launches the executable inside `dist/diri.app` directly
with the deterministic stress sidebar fixture, records the exact PID it owns,
waits 30 seconds for startup and Metal resource retirement to settle, then
measures:

- physical footprint from `vmmap -summary`;
- mean and peak idle CPU across interval samples from `top`;
- both the normal 1100×700 window and the retained 1800×1100 stress switch.

The fixture contains working, starting, and needs-input status rows—the states
that triggered the original decorative repaint loop—without attaching to or
resizing the user's selected live PTY. Preview mode also uses inert store and
updater handles: it does not connect to the daemon, scan transcripts, or check
the network. Pass `--live-daemon` only for an intentional follow-up measurement
of the real local session set.

It does not use `pkill`, name-based termination, or touch an existing Diri. On
cleanup it verifies the original process start time and terminates only the PID
it launched. Default release ceilings are 80 MB normal, 140 MB large, 0.75%
mean idle CPU, and 1% peak idle CPU. These bounds retain practical machine
variance while catching a meaningful regression well before the reproduced
~500 MB / ~29% failure.

After packaging, run the same gate the release script runs:

```sh
diri/scripts/perf-gate.sh --app diri/dist/diri.app --scenario all
```

Budgets are configurable for deliberate tightening:

```sh
DIRI_PERF_NORMAL_MAX_MB=80 \
DIRI_PERF_LARGE_MAX_MB=140 \
DIRI_PERF_IDLE_AVG_CPU=0.75 \
DIRI_PERF_IDLE_PEAK_CPU=1 \
  diri/scripts/perf-gate.sh --app diri/dist/diri.app
```

`diri/scripts/release.sh` runs this after the final bundle is signed, notarized,
and stapled but before copying or publishing artifacts. `SKIP_PERF_GATE=1` is
an explicit escape hatch for a non-GUI release host; any such release needs the
same packaged bundle measured manually on a Mac before publication.

For final release sign-off, first run the deterministic gate. Then close
unrelated high-load apps and optionally repeat with `--live-daemon` to sample
the normal local session set. Record the printed PID, footprint, average CPU,
peak CPU, macOS version, and hardware next to the release notes. Do not reuse
historical development-binary numbers.

## Validation

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
diri/scripts/perf-gate.sh --app diri/dist/diri.app --scenario all
```

The packaged probe is the release acceptance authority; the historical command
results above apply only to the dated T16 sample.
