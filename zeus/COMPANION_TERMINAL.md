# Companion terminal v1 (#75)

V1 is command-oriented with read-only terminal snapshots. It does not advertise
full interactive terminal parity. The gateway uses `zeus-client` and the local
Engine; a phone never opens a Holder channel or sends `remote_pty` messages.
The external schema, authentication, scopes, revocation, listener policy and
gateway limits belong to #76/#73. The types in `zeus-proto::terminal` are local
IPC DTOs, not an external RPC passthrough.

## Ownership and commands

The Engine Session owns one controller: `{id, label, role}` plus an opaque
`{incarnation, generation}` epoch. The incarnation is a random Engine Session
nonce, distinct from the process/Holder incarnation. Engine restart or Session
replacement creates a new nonce; SSH reconnect retains it. Nothing is added to
Holder state, its one-controller protocol, or persisted session records.

Reading does not acquire control, wake the process, or resize the PTY. Take
Control compares the observed epoch and requires explicit takeover when occupied.
The comparison, revocation and replacement occur under the Registry lock, which
is also held through input/lifecycle effects. Concurrent takeovers using the same
observation have exactly one winner. Release advances the epoch. A mobile network
disconnect retains ownership until explicit release, revocation or takeover:
there is no timeout-based surprise desktop writer and no automatic reacquisition.
The gateway must explicitly release each owned session when revoking a device.

Each text command carries the current epoch, authenticated owner ID, and the next
`commandSeq`. The Engine consumes the sequence before attempting I/O, including
failed or uncertain delivery. Repeated, skipped and stale commands fail closed.
The gateway must never automatically retry a command after losing its reply;
read a fresh snapshot and show unconfirmed delivery instead. This gives at-most-
once attempts, not a claim of exactly-once execution by the Agent. State survives
a gateway restart; an Engine restart invalidates all old commands. No prompt
payload is retained for deduplication or included in Debug implementations.

V1 allows bounded text and submit, with bracketed paste when enabled. Escape and
other terminal controls are rejected. Multiline/tab text requires bracketed paste
so it cannot unexpectedly execute multiple shell commands. Hibernated, deferred,
exited or reconnecting sessions do not queue Companion input for later delivery.
Raw keyboard, mouse, focus, signal and resize are not Companion capabilities.
Desktop raw input uses its negotiated, epoch-checked attachment envelope.

The gateway must map a paired device to the owner ID/label itself, never accept an
arbitrary owner from the phone. Epochs identify ordering; they are not credentials.
Trusted internal automation remains a separate authority and is not exposed by
the gateway. Unfenced legacy send-text/resize/kill is blocked while a mobile device
owns the terminal. Fenced lifecycle operations must validate under the Registry
lock and keep it through the effect.

## Snapshot, reconnect and viewport

`terminal.snapshot` negotiates protocol 1 and returns the current full visible RLE
grid, complete cursor/modes, owner/epoch, exit fact and a snapshot cursor. A request
with the exact current cursor omits the grid. A missing, old, future or different-
incarnation cursor always receives a full snapshot. Modes and control facts are
returned even if the grid is unchanged. Snapshot sequences order distinct sampled
terminal states, not PTY byte offsets; there is no terminal replay window in v1.

The snapshot is copied under the existing screen/mirror lock without consuming
desktop dirty rows. It never spins until output becomes quiet. Local and remote
sessions use the same Engine lease and snapshot DTO. Mobile rotation scales or
crops the current grid; only the desktop controller changes PTY dimensions.

Snapshots allow dimensions 1..512, at most 32,768 cells and 1 MiB encoded bytes.
Geometry/cursor/row widths are validated before client row allocation. Requests
use bounded NDJSON and responses have a socket write deadline. On-demand scrollback
allows at most 128 rows and 32,768 cells, capped at 1 MiB per response; it never
changes the shared scroll position. The existing Holder scrollback bound remains.
Text allows at most 16 KiB. Controller IDs/labels allow at most 128 bytes each.

There is no per-viewer terminal queue, subscription or replay ring in this slice:
one snapshot request gets one current response. A slow/disconnected mobile client
cannot retain stale diffs, grow a terminal queue, or block the PTY reader; it
reconnects and requests a fresh full snapshot. The gateway must bound active
connections, concurrent requests and outbound queues, discard superseded pending
snapshots and reseed after overflow. It must not schedule an unbounded snapshot
poller. With no request there is no Companion grid construction or heartbeat.

## Deferred interactive channel

Sequenced grid deltas, resumable replay, phone PTY resize, mouse/focus/keyboard
events and signals remain unadvertised until full-control correctness is proven.
Required proof includes alternate screen and every supported input mode, stale
epoch rejection for every mutation, bounded overflow/reseed behavior, consistent
geometry, process exit/failure, Engine restart, local/fake-remote parity, and the
existing release-mode Helper latency gates. This implementation does not broaden
the Remote Holder baseline to multiple observers or mobile orchestration.
