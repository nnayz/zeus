# Roadmap

Direction only — not a release calendar.

Zeus is the attention and lifecycle layer for a fleet of coding agents. It sits
above Claude Code, Codex, Cursor, Gemini, and shells: a native macOS control
plane that keeps real PTYs alive, tells you which sessions actually need you,
and is where you accept the work. It is not an IDE, not a model, and not a
hosted agent.

The durable work is:

1. **Status you can leave the room on.** Zeus reads the PTY grid, hooks, process
   facts, and title/progress, then reduces them to working / needs-you / done.
   Status is not a vendor API. The manifest catalog and the status reducer are
   how that knowledge ships.
2. **Sessions that survive the app, the Engine, and SSH.** Holders own the PTY.
   Quitting the window, restarting the daemon, or dropping a remote connection
   must not kill the agent. Remotely, Zeus bootstraps a Helper over `ssh -T`
   and owns the remote PTY — no `tmux`, no preinstalled service.
3. **An agent-agnostic control plane.** A new agent is a manifest: spawn,
   resume, approve/deny keys, screen rules. Worktrees isolate git collisions.
   MCP lets one agent spawn, wait on, prompt, and read another.

Everything on this page either deepens those three, or is explicit work we will
not take.

## Now

Harden what already makes Zeus usable as a fleet cockpit.

- **Session persistence and Engine upgrades that stay boring and recoverable.**
  Holders keep the PTY and agent process tree across app quit, Engine crash,
  and Engine upgrade. Screen checkpoints stay a cache: a bad file is a miss,
  not a lost session. Upgrade and recovery paths must restore the same process
  identity and the same terminal snapshot.
- **Broader first-class agent manifests and status detection.** Claude Code and
  Codex already have first-class status and resume. Other catalog agents should
  move toward the same: working / blocked-permission / blocked-question / idle /
  exited, with anti-flicker, blocker arbitration, and captured prompt text.
  Status quality is the product. A spinner that lies is worse than no spinner.
- **Stronger release supply-chain checks.** Signing, notarization, and the
  update feed stay a release gate. The updater already verifies SHA-256,
  signature, Team ID, bundle id, notarization, and refuses downgrades.

## Next

Make “I can run more agents than I can watch” true.

### Needs-you inbox

Status detection is the engine. The inbox is the product.

One queue across every live session — local and remote — of the things that
actually need a human:

- permission prompts, with the captured prompt text and a risk label
- questions and confirmations
- failures and unexpected exits
- done-with-diff (the session finished and produced a change)

Triage is keyboard-first: approve, deny, jump to the session, snooze, or
dismiss. One keystroke should clear a permission the way Superhuman clears
mail. The sidebar remains the map of the fleet; the inbox is the work.

This is not a notification dump and not a second chat transcript. If the
reducer cannot name the blocker, the inbox must not invent one.

### Accept-the-work surface

Agents produce. Zeus is where you accept.

Each session that edits a repo should have a review cockpit bound to its
worktree: live diff, stage, commit, PR, conflict view, this-agent-branch vs
main. Existing git review and PR monitoring fold into that surface rather than
living as separate toys.

A fleet without a merge surface is expensive chaos. Worktrees stay the
isolation mechanism, not a security boundary: they prevent edit collisions so
several agents can work the same project at once. Treat each worktree as a
normal checkout — commit or move changes before deleting it.

### Status catalog

Whoever has the best “is it stuck?” detection owns the category.

- First-class manifests for every serious CLI agent, kept current as their TUIs
  change.
- Hooks where the agent has them; screen rules where it does not.
- A new agent remains a JSON file: spawn, resume, approve/deny, screen rules.
  Adding a catalog entry must not require Engine code.
- Missing or stale rules fail soft (bare login shell / running-or-exited),
  never invent a confident wrong state.

This is a data moat that can grow without telemetry. Community or
auto-updated rules are in scope; shipping session contents off-box is not.

### Remote as a first-class machine

The Remote Holder is a rare technical asset. “Run this on the 64-core box”
should be as cheap as local: one host picker, the same status, the same inbox,
the same persistence.

- Clearer remote-host and remote-node setup and diagnostics. Probe, bootstrap,
  persistence capability (`native-detach` / `user-supervisor` / `non-persistent`),
  and structured transport errors must be explainable in the UI, not only in
  logs.
- Instant move and fork between laptop and an enrolled `zeus-node` (the
  existing handoff coordinator): checkpoint at a turn boundary, transfer only
  missing blobs, restore into quarantine, provider-native resume or fork,
  commit the location lease. The live workspace is never overwritten during
  staging.
- The Mac stays the glass. Compute lives wherever it is cheapest or most
  persistent. SSH remains the authenticated byte transport; Zeus never
  reintroduces `tmux` as a remote session fabric.

### Human-gated agent-to-agent

MCP spawn / list / status / send / wait / read / release is the seed of an OS
for agent work. Unsupervised swarms are a liability. Supervised swarms are the
wedge.

Make orchestration a first-class object, not a hidden tool call:

- named jobs (“three worktree-isolated agents, stop at PR”)
- parent/child lineage visible in the sidebar and inbox
- allowlists for who may spawn whom, on which host, in which project
- budgets (see fleet cost below)
- a paper trail of who spawned whom and what was sent

Do not add a multi-session remote supervisor or turn `zeus-remote` into a
second Engine. Orchestration stays in the local Engine; the Helper stays a
Helper.

## Later

These compound the same loop once the inbox, accept surface, and status
catalog are trustworthy.

### Notifications that close the laptop

Desktop — and later phone — alerts only on `needs-you` or `done`, carrying the
actual blocker text. Quiet hours. Per-project and per-agent mute. Persistence
plus a page is how Zeus becomes all-day infrastructure instead of a window you
stare at.

This is not a firehose of token activity. Working sessions stay silent.

### Fleet memory

Search across session titles, captured prompts, and terminal history: “what
did the Codex on `auth-rewrite` decide about JWT?” Jump to that screen offset.
A week of parallel agents becomes recoverable memory, not forty dead tabs.

Replay stays local. Terminal logs already can contain secrets; search must not
create a new exfiltration path, a Zeus-operated index, or a requirement to
keep unbounded raw logs.

### Cost and quota as control

Usage accounting and the resource governor already exist. The feature is
policy, not another chart:

- pause a provider when weekly quota hits a threshold
- cap a project or profile at a daily spend
- freeze the oldest idle session first when the machine budget is hit (the
  governor already hibernates unattended idle sessions)
- show spend and quota next to status, including merged totals from enrolled
  nodes

Parallel agents die on surprise bills. The orchestrator that prevents that
becomes required. An unreachable node must never block local numbers.

### Read-only observers

The current baseline is exactly one live attach/controller. A new attach
revokes the old one. That remains correct for input.

A later enhancement is read-only spectators: a teammate or a second device
watches a live session without resizing the PTY or injecting keystrokes. Share
a session, not a screenshot. This is how Zeus becomes a team product without
becoming a hosted SaaS. Multi-controller input and a Zeus-operated relay stay
out of scope.

### Cross-platform Engine

The desktop app is macOS. The Engine, Holder, and Remote Helper are already
Rust. Broader Engine coverage (Linux as an execution host and, later, as a
headless cockpit) extends the same ownership model. It does not reopen the
transport, reintroduce Swift, or make a hosted Zeus account a prerequisite.

## Distribution

- Signed, notarized GitHub Releases
- Homebrew cask (monorepo tap):
  `brew tap nnayz/zeus https://github.com/nnayz/zeus.git && brew install --cask nnayz/zeus/zeus`
- Deeper end-to-end coverage for updates and session recovery, including the
  existing remote soak and Helper-native probe gates

## Not planned

These fights are already lost, or they contradict the product.

- **A hosted Zeus account, analytics, or telemetry service.** There is no
  Zeus-operated relay for terminal contents or session history.
- **Treating agent processes as a security sandbox.** Zeus launches powerful
  local tools with the user’s privileges. Isolation is an OS account, VM, or
  container — not a Zeus feature.
- **Becoming an IDE.** Zed, Cursor, and VS Code win the editor. Zeus owns
  fleet attention, PTY lifecycle, and accept-the-work.
- **Becoming an agent or a model vendor.** Claude Code, Codex, Gemini, and the
  rest win the conversation. Zeus stays above them.
- **A Zeus Cloud that runs your agents for you.** Remote execution is your
  SSH host or your enrolled node, on your credentials.
- **Reintroducing `tmux`, `screen`, or `zellij` as the remote session
  transport.** The Remote Holder is the only remote transport.
- **A multi-session remote Zeus supervisor, or growing `zeus-remote` into a
  second Engine.** Hooks, MCP forwarding, artifacts, ports, usage, handoff,
  checkpoints, and resource governance stay local-Engine concerns unless this
  document and `zeus/REMOTE_PORT.md` are revised together.
- **Requiring Node.js, Python, or a preinstalled Zeus service on the remote
  host** for the default SSH path.
- **Host-wide configuration:** no `sudo`, package installation, PAM/sshd
  changes, system services, or persistent user units as a setup step.
