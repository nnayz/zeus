# Idle sessions and resource limits

## When this is useful

You are running more sessions than you are watching, the laptop is small,
or you want the fleet to stay overnight without holding RAM for terminals
you are not looking at.

## Prerequisites

- **Settings → Resources** (`⌘,`).
- Sessions named well enough that tomorrow's sidebar still makes sense
  (`⌘R`).
- Writers already isolated if they edit. Hibernate does not stop two agents
  in one checkout from clobbering files. See
  [Parallel agents in worktrees](parallel-worktrees.md).

## Steps

1. Choose **Hibernate idle sessions**: Off, 5 minutes, 15 minutes (the
   default), 30 minutes, or 1 hour. A frozen session is not killed. Opening
   it wakes the same process on the same screen. The row shows
   **Hibernated**.
2. Set **Memory limit** to 2, 4, 6 (the default), or 8 GB per session.
   Crossing that limit freezes the session the same way.
3. Before you leave:

   - Name every live session.
   - Give each writer its own worktree.
   - Tell the lead what done means, and that it must not approve destructive
     prompts.
   - Leave the menu bar extra enabled so the "needs you" count is visible
     without focusing Taskmaster.

4. Optional: **Start Taskmaster at login** only puts the window back after you
   sign in. The Engine was already keeping the sessions. **Gentle status
   chimes** cover needs-input, completion, and memory pauses, not ordinary
   token traffic.
5. In the morning, press `⌘⇧J` for sessions that are blocked. Open any
   hibernated row you still need. It wakes in place.
6. Archive rows you have merged or abandoned (`⌘⇧W`). Then open the
   worktrees sheet (`⌥⌘W`) and remove trees that are stale, merged, not
   dirty, and not `main`. Taskmaster asks you to confirm.
7. Use `⌘W` only when you mean to close that session. With **Confirm before
   closing a session** on, Taskmaster asks before it kills a running process.
   `⌘Q` quits the app and leaves the PTYs with the Engine.

## Expected result

- Idle sessions freeze instead of accumulating RAM, within the per-session
  limit you set.
- Opening a hibernated row shows the same terminal you left.
- Nothing was killed just to enforce the limit.
- The morning inbox is `⌘⇧J`, not a reread of every scrollback.

## Pitfalls

- The memory limit is per session. 6 GB does not mean 6 GB for the whole
  fleet.
- A session in **Needs input** is not idle. Hibernate will not answer a
  permission prompt. Destructive confirms still wait for you.
- Do not remove a worktree that still has uncommitted work you want. The
  sheet refuses dirty trees; `taskmaster worktree remove --force` does not.
- Archive is not resume, and it is not a merge.
- Closing the lid does not create a remote multiplexer. Whether a remote
  process survives disconnect is a Details fact. See
  [Local or remote](local-or-remote.md).

The settings themselves are in [Settings](../settings.md). The overnight
shape of a fleet is also in [Fleet patterns](../fleet.md).
