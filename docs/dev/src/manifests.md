# Agent manifests, hooks, and MCP

What users see (catalog, status colors, resume, hosted versus a plain
shell) is [Agents and status](../../src/agents.md) in the User Guide. This page
is where that behavior is stored.

## Manifests

One JSON file per CLI under `taskmaster/crates/taskmaster-engine/manifests/`. A manifest
describes how to spawn the CLI, how to resume it, which keys approve or deny
a prompt, and the screen rules that decide working, waiting, or done.

The launcher, `⌘N`, `⌘T`, and the command palette read that catalog. A
missing binary stays visible with an install hint. A missing manifest does
not error at runtime. Taskmaster launches that command as a bare login shell and
reports only running or exited. Do not delete manifests to tidy the list.

Claude Code and Codex have the richest status and resume rules. Do not
invent a blocker for an agent whose manifest cannot name one.

## Hooks

`taskmaster hook` and `taskmaster notify` are fail-open forwarders for Claude hooks and
Codex notify. They let those CLIs poke Taskmaster. They are not an interactive UI.
Operator-facing command names are on the User Guide
[Command line](../../src/cli.md) page.

## MCP

Hosted sessions (anything Taskmaster launched from a manifest) receive a
per-session `taskmaster` MCP server. Taskmaster writes that overlay. Users do not paste
it into a dotfile for the default path. A plain `⌥⌘T` shell does not get
the server.

Codex's built-in multi-agent spawn is disabled while Taskmaster is hosting it, so
those inner workers do not bypass the sidebar. `spawn_agent` is the spawn
that creates a session.

The tool list users and leads rely on is documented in
[Orchestration](../../src/orchestration.md). `taskmaster mcp-tools` prints the live
catalog from a running Engine. Implementation of those tools stays in the
Engine and `taskmaster-cli`, not in this page.

`spawn_agent` does not accept a remote `host`. Remote children are started
on a session that is already on that host, or with
`taskmaster session spawn --host`.
