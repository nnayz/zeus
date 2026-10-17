# A session is waiting

## When this is useful

A sidebar row has stopped making progress and you need to know whether it
wants a permission, an answer, or has already finished.

## Prerequisites

- The session is listed in the sidebar.
- Optional: **Gentle status chimes** in **Settings → General**. Chimes cover
  needs-input, completion, and memory pauses. They stay quiet while a
  session is merely working.

## Steps

1. Read the status, not the spinner. Taskmaster reduces the grid, title, progress,
   hooks, and process facts to:

   | State | Meaning |
   |-------|---------|
   | **Working** | The agent is doing something. It is not blocked on you. |
   | **Needs input** | A permission, a question, or a confirm. Destructive prompts use a sharper color. |
   | **Done** | It finished work you have not opened yet. |
   | **Idle** | Quiet, and you have already seen it. |
   | **Hibernated** | Frozen to save memory. Opening it wakes the same process. This is not a question. |
   | **Ended** | The process exited. Resume only if that agent supports it. |

2. Press `⌘⇧J`. It walks only sessions that need input. The menu bar extra
   shows that count without bringing the window forward.
3. Open the row. Claude Code and Codex can distinguish a blocked permission
   from a blocked question, and they can resume. Other catalog agents report
   only what their manifest knows. If Taskmaster cannot name the blocker, it leaves
   the row quiet instead of inventing one.
4. Answer in that terminal. If a lead spawned the child, you can instead tell
   the lead to `send_prompt` into it. See
   [Delegate and follow child sessions](delegate.md).
5. From a shell, when the window is the wrong tool:

   ```sh
   taskmaster doctor
   taskmaster session list
   taskmaster session get <id>
   taskmaster session wait <id> --until needs-input --timeout 600
   ```

   `taskmaster doctor` is the right first command when the app cannot connect or
   a spawn never starts. It does not answer an agent prompt for you.

## Expected result

- `⌘⇧J` lands on a session that is actually blocked on you.
- Approving or answering returns that session to **Working**.
- A session Taskmaster cannot classify does not grow a fake "waiting" badge.

## Pitfalls

- **Hibernated** looks stuck and is not. Open the row. Limits are in
  [Idle sessions and resource limits](idle-and-limits.md).
- **Ended** will not continue in place unless that agent supports resume.
  Trust resume for Claude Code and Codex. Archiving is history, not resume.
- `⌘W` closes the selected session and can kill a running process. Turn on
  **Confirm before closing a session** if that is easy to hit by mistake.
- Do not paste an SSH password or host-key answer into the agent. Remote
  prompts use the askpass helper
  ([Local or remote](local-or-remote.md)).
- A hosted Codex "subagent" that never appears in the sidebar is still
  inside one PTY. It is not a child session waiting in the tree.

Status rules are in [Agents and status](../agents.md). Commands are in
[Command line](../cli.md).
