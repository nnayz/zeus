# Privacy

Taskmaster has no account system, advertising, analytics, or telemetry. There is no
Taskmaster-operated service that receives your terminal contents or session history.

## Data stored on your Mac

Taskmaster stores session state, terminal replay logs, host configuration, preferences,
usage summaries, and search/index data under these locations:

- `~/Library/Application Support/Taskmaster`
- `~/Library/Application Support/taskmaster`
- `~/Library/Caches/taskmaster/updates`

Terminal logs can contain prompts, command output, repository paths, and secrets
printed by a process. Treat them as sensitive and redact them before sharing
diagnostics. Archiving can intentionally preserve session metadata. Deleting the
directories above removes all Taskmaster-managed local data after Taskmaster and its daemon
are stopped.

## Network activity

Taskmaster connects to GitHub Releases to check for and download updates. It may also
make network connections when you explicitly use remote hosts, PR monitoring,
browser automation, or a tool/agent that uses the network. Those tools and
services have their own privacy practices. Taskmaster does not proxy their traffic
through a Taskmaster-operated server.

Remote-node credentials remain in the mechanisms you configure (for example,
SSH configuration and your keychain); they are not sent to Taskmaster.

## Process access

Taskmaster is not sandboxed because its core function is to launch shells and coding
agents, create worktrees, and communicate with local tools. Child processes run
with your macOS user privileges and may inherit environment variables. Only run
agents and MCP servers you trust, and review their permissions separately.

For vulnerability reports, email **[hi@nasrul.info](mailto:hi@nasrul.info)**
(see the [security policy](security-policy.md)).
