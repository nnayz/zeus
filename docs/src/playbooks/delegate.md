# Delegate a task and follow child sessions

## When this is useful

You want one session to talk to, and other real Taskmaster sessions to do the
work. Children should show up under the lead in the sidebar, each with its
own terminal and Review diff.

## Prerequisites

- A hosted agent: Claude Code, Codex, Grok, OpenCode, Gemini, or Cursor.
  Taskmaster writes the per-session MCP overlay at launch. You do not paste MCP
  JSON into a dotfile for that path.
- A plain terminal (`⌥⌘T`) does not get the `taskmaster` MCP server. Do not use it
  as the lead.
- You trust the lead. A spawned agent runs as your macOS user. Read what it
  asks before you approve it.

## Steps

1. Start the lead with `⌘T` or `⌘N` in the repository. Rename it `lead`
   (`⌘R`).
2. Tell it the outcome in the terminal. You do not have to name the tools.
   For example:

   > Spawn a Codex in a fresh worktree on the auth rewrite. Wait until it is
   > done or needs me. If it asks for a destructive permission, ping me
   > instead of approving. Then spawn a second Codex to review the diff and
   > stop at a PR description. Do not merge.

3. Expect the lead to `spawn_agent`, wait, and `read_output`. Nested rows
   appear under the lead. Collapse the lead if the tree is noisy. Select any
   row to open that terminal.
4. Name the children in the assignment (`auth-rewrite`, `reviewer`) so the
   sidebar stays scannable.
5. When a child blocks, `⌘⇧J` selects it. Answer in that terminal, or tell
   the lead to `send_prompt`.
6. Use `⌘⇧O` when you want the board of live sessions instead of the tree.
7. Archive a finished row with `⌘⇧W` when you only need history. Archiving
   is not resume. Ending the process is `release_agent` (the row stays) or
   closing the session.

The same conductor can be a script. The CLI `--prompt` flag injects text
once the agent can take it, which is not how MCP `spawn_agent` behaves:

```sh
taskmaster session spawn codex --cwd ~/src/mldrills --worktree \
  --title auth-rewrite \
  --prompt "Implement the JWT refresh plan in AGENTS.md. Stop at a passing test."

taskmaster session wait <id> --until done --timeout 1200
taskmaster session read <id> --source output --lines 80
```

`taskmaster mcp-tools` prints the live catalog.

## Expected result

- The lead stays the session you talk to. Children are separate sidebar rows
  under it.
- Each child has its own PTY and its own Review diff.
- After a wait, the lead's summary comes from `read_output`, not from a
  quiet terminal it never read.

## Pitfalls

- Codex's built-in multi-agent spawn is off while Taskmaster is hosting it. Those
  inner workers never become sidebar sessions. `spawn_agent` is the path
  that does. Do not tell a hosted Codex to "use its subagents."
- MCP `spawn_agent` returns `pendingPrompt`. The Engine does not type it.
  The lead should `wait_for_agent` until the child is idle, then
  `send_prompt`. `wait_for_agent` defaults to five minutes; say so when the
  job is longer.
- A local lead cannot MCP-spawn onto a remote host. `host` is not an MCP
  argument. Start the remote session from the palette or
  `taskmaster session spawn --host`, then orchestrate children on that machine.
- Do not treat a worktree as a sandbox.
- Skipping Review leaves you with branches you have not read. The accept
  step is [Review changes and open a pull request](review-pull-request.md).

Tool names and wait modes are listed on [Orchestration](../orchestration.md).
[Fleet patterns](../fleet.md) shows splits such as implementer, reviewer, and
shell.
