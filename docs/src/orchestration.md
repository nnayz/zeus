# Orchestration

Zeus is most itself when one agent runs others. A hosted session can spawn a
child, wait until it is done or stuck, read the output, answer a prompt, and
release it. Those children are real Zeus sessions: sidebar rows, inspector
diffs, workflow-tree nodes.

This is powerful. A spawned agent runs as your macOS user. Only talk to
agents you trust, and read what they ask before you approve it.

## You do not configure MCP by hand

For Claude Code, Codex, Grok, OpenCode, Gemini, and Cursor, Zeus writes a
per-session MCP overlay when it launches the process. The agent sees a
`zeus` server. You do not paste JSON into a dotfile for the default path.

Codex's built-in multi-agent spawn is turned off on purpose while Zeus is
hosting it. Those inner workers never become sidebar sessions. `spawn_agent`
is the path that does.

A plain `⌥⌘T` shell does not get this server.

## Tools the lead can call

| Tool | When to use it |
|------|----------------|
| `spawn_agent` | Open a **new** Zeus session nested under this one. Pass `kind`, `cwd`, optional `worktree`, `name`, and `prompt`. `worktree` is local only. This is the only spawn that shows up in the UI. |
| `list_agents` | Survey the fleet: id, kind, title, status, parent, cwd. |
| `list_children` | Just the sessions this lead spawned. |
| `get_status` | One session: working, idle, needsInput (with detail), or exited. |
| `send_prompt` | Type into a session and, by default, submit. Use it for follow-ups and for answering a blocker. |
| `wait_for_agent` | Block until `done`, `needsInput`, `exited`, or `any`. |
| `wait_for_children` | Block until every child of this lead is done. |
| `read_output` | Tail the PTY after a wait, so the lead can summarize or critique. |
| `release_agent` | End a session and kill its process tree. The row stays in the list. |
| `create_worktree` / `list_worktrees` / `remove_worktree` | Isolate parallel git work. |
| `whoami` | The calling session's id, kind, cwd, and parent. |

`zeus mcp-tools` prints the live catalog. `zeus mcp-call --tool list_agents`
is the one-shot CLI for the same socket.

## How to ask

You do not need to name the tools. Talk to the lead in the terminal:

> Spawn a Codex in a fresh worktree on the auth rewrite. Wait until it is
> done or needs me. If it asks for a destructive permission, ping me instead
> of approving. Then spawn a second Codex to review the diff and stop at a
> PR description. Do not merge.

The lead should `spawn_agent` twice, `wait_for_agent` (or `wait_for_children`),
and `read_output`. You will see two nested rows and a tree with three nodes.

If a child is blocked, `⌘⇧J` lands you on it. Answer in that terminal, or
tell the lead to `send_prompt`.

## Shape of a good spawn

- **Name the session.** `name` becomes the sidebar title (`auth-rewrite`,
  `reviewer`, `load-test`).
- **Give it a worktree** when it will edit. Share a cwd only when the child
  is a shell that should see the parent's files.
- **Send the assignment after the child is ready.** MCP `spawn_agent` returns
  `pendingPrompt`. The Engine does not type it. The lead should
  `wait_for_agent` (or wait until the child is idle) and then `send_prompt`.
  The CLI `--prompt` flag is different: `zeus session spawn` injects that
  text once the agent can take it.
- **Wait with a timeout.** `wait_for_agent` defaults to five minutes. Long
  jobs should say so.
- **Read before you praise.** `read_output` is how the lead learns what
  happened. Do not let it invent a victory from a quiet PTY.

## CLI as a second conductor

Anything MCP can do, the `zeus` CLI can do from a script or from another
terminal:

```sh
zeus session spawn codex --cwd ~/src/mldrills --worktree \
  --title auth-rewrite \
  --prompt "Implement the JWT refresh plan in AGENTS.md. Stop at a passing test."

zeus session wait <id> --until done --timeout 1200
zeus session read <id> --source output --lines 80
```

See [Command line](cli.md).

## What not to do

- Do not tell Codex to "use its subagents" while it is hosted in Zeus. Those
  stay inside one PTY. The sidebar will look empty and you will have lost
  the plot.
- Do not expect a local lead to MCP-spawn onto a remote host. `host` is not
  a supported MCP argument yet. Start remote sessions from the palette or
  `zeus session spawn --host`, then orchestrate children on that same
  machine.
- Do not confuse a worktree with a sandbox. Children still have your keys.
- Do not skip Review. Orchestration without an accept step is just more
  unmerged branches.
