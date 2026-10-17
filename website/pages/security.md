# Security

## Current builds are not notarized

[v{{version}}](/releases/{{version}}/) is ad-hoc signed. Apple has not notarized it. macOS Gatekeeper will warn or block a double-click. Follow the [illustrated install guide](/install/) to create a one-app exception without disabling Gatekeeper.

The in-app updater requires Developer ID and notarization. It will not offer these builds. Later signed releases will say so here and on the release page.

Download only from [GitHub Releases]({{releases}}) under `nnayz/taskmaster`. Treat any other binary as untrusted.

## Taskmaster is not a sandbox

Taskmaster launches shells, coding agents, MCP tools, and optional remote commands with your macOS user privileges. It reduces orchestration mistakes. It does not inspect or approve each operation those tools perform.

Worktrees avoid edit collisions. They are not a security boundary. For untrusted code, use a dedicated OS account, VM, or container.

Remote sessions run under the SSH account you configure. Taskmaster does not add a separate authorization layer.

## Data

No Taskmaster account, analytics, or telemetry. Session state and terminal replay live on your Mac under Application Support. Logs can contain prompts, paths, and secrets printed by a process.

## Report a vulnerability

Email [hi@nasrul.info](mailto:hi@nasrul.info?subject=Taskmaster%20security) with the subject line `Taskmaster security`.

Include Taskmaster version, macOS version, a minimal reproduction, and expected impact.

Do not attach private terminal output, tokens, or personal paths unless they are required, and mark that mail as confidential. Acknowledgement should arrive within seven days.

In scope: permission-boundary bypasses, unsafe update or IPC behavior, credential disclosure, session isolation failures, unintended remote execution. A tool doing something you authorized is not itself a Taskmaster vulnerability.
