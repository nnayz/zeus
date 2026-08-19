# Fleet patterns

Zeus is for the week you have more bets than attention. These patterns use
only what the app already does. Mix them.

## The lead and two specialists

One session you talk to. It does not edit much. It hires.

1. `⌘T` a Claude or Codex in the repo, titled `lead`.
2. Tell it the outcome, the constraints, and that it must spawn specialists
   into worktrees rather than touching `main`.
3. Open **Tree**. You should see the lead above a row of marks.
4. Drink coffee. `⌘⇧J` only when something is red.

A useful split:

- **Implementer** (Codex or Claude) owns the patch.
- **Reviewer** (a different kind, Grok or OpenCode) reads `read_output` and
  the diff, never the same worktree as the writer if you can help it.
- **Shell** (`kind: shell`) runs the test command in the implementer's tree.

The lead calls `wait_for_children`, then writes you a briefing. You accept
or reject in **Review**.

## Three spikes, one merge

You do not know which approach is right. That is a fleet, not a meeting.

Spawn three named worktree sessions (`spike-index`, `spike-cache`,
`spike-rewrite`) with the same prompt and a hard stop: "do not open a PR.
Leave a summary at the top of the diff." When they go idle, sit in session
overview (`⌘⇧O`) and open Review on each. Keep one tree, delete the other
two after you have stolen the good ideas.

The tree view is the scoreboard. Ended spikes stay on the board until you
archive them (`⌘⇧W`).

## Local glass, remote muscle

The Mac is where you look. The 64-core box is where tokens burn.

Add the host in Settings → Remote. From `⌘K` run **New Codex on Forge**
(or whatever you named it). Status, resume, and Review still happen in
this window. The Helper on the host owns the PTY. There is no `tmux` to
babysit.

A session that is already on Forge can spawn children there (same host,
same MCP). A lead that is still on the laptop cannot MCP-hire onto Forge
yet. Start the remote workers from the palette or
`zeus session spawn --host forge`, then talk to them from the tree.

If you enrolled a `zeus-node`, use handoff to move a Codex or Claude
thread to the VPS without copying `auth.json`. See
[Remote nodes](remote-nodes.md).

## Overnight, not on fire

Settings: start at login, gentle chimes on, hibernate idle sessions after
15 or 30 minutes, memory limit at 6 GB.

Before you close the lid:

- Name every live session.
- Make sure each writer has its own worktree.
- Tell the lead what "done" means, and that it must not approve destructive
  prompts.
- Leave the menu bar extra enabled.

In the morning, `⌘⇧J` is the inbox you actually have today. Hibernated rows
wake when you open them. Nothing was killed to save RAM.

## Research / implement split

Grok (or Gemini) in the repo with `worktree: false` and a prompt that
forbids editing: "read, cite files, propose a plan, do not write patches."
Codex or Claude in a worktree with that plan pasted as `prompt`.

The researcher can `send_prompt` follow-ups. The implementer never has to
see the web. You stay on the implementer's Review tab.

## Pair a shell with every writer

`⌘J` puts a login shell under the current agent. Use it for `git log`,
`just test`, `htop`. The agent keeps its TUI. You stop pasting command
output into the chat because you can see the repo yourself.

For a child that should be scriptable, spawn `kind: shell` from MCP instead
so the lead can `send_prompt` a command and `wait_for_agent` on exit.

## Codex-only keyboard

If Codex is how you think, set it as the default agent and live on `⌘T` and
`⌘⇧N`. Keep Claude a palette search away for the jobs it is better at
(long-context review, MCP-heavy leads). The catalog is not a loyalty club.

## Conduct, then accept

A loop that scales:

1. **Tree** to see who is alive.
2. **Needs you** (`⌘⇧J`) to unblock.
3. **Review** to read the diff in the same worktree.
4. **Archive** the ones you have merged or abandoned.
5. **Worktrees sheet** (`⌥⌘W`) to delete the empty apartments.

If you skip step 3 you do not have a fleet. You have a garden of branches.

## Naming

Sidebar titles are load-bearing. `Claude Code 3` is how you get lost.
`lead`, `auth-rewrite`, `reviewer`, `forge-load` is how you scan.

`⌘R` is faster than regretting it at 1 a.m.
