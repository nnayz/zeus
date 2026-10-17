# Choose local or remote execution and reconnect

## When this is useful

The Mac is where you look. You need to decide whether the agent runs on
that Mac or on another machine you can SSH to, and you need the same
session again after you quit Taskmaster or the connection drops.

## Prerequisites

- Local: a project open in Taskmaster, and the agent CLI on `PATH`.
- Remote: the host saved under **Settings → Remote** (`⌘,`). Set a name
  (the palette label), an SSH destination or `~/.ssh/config` alias, and a
  default remote cwd.
- Prefer a dedicated non-admin account and narrowly scoped credentials.
- Most hosts should stay SSH-only. Add a first-party node only when you
  need per-machine Claude or Codex logins, merged usage, or a move/fork of
  a live thread. That setup is [Remote nodes](../remote-nodes.md).

## Steps

1. Stay local when the checkout and the credentials you want are already on
   this Mac. `⌘N` or `⌘T` starts a session in the open project. The palette
   row does not name a host.
2. Use a remote host when the work should run somewhere else. Open `⌘K` and
   search the host name, for example **New Codex on Forge**.
   Or from the CLI:

   ```sh
   taskmaster session spawn codex --host forge --cwd ~/code/mldrills
   ```

3. The first successful connection uploads a small Helper (`taskmaster-remote`).
   SSH is only the authenticated pipe. The Helper owns the agent PTY. You
   do not install `tmux`, Node.js, Python, or a Taskmaster service on the host.
   OpenSSH config and a short-lived ControlMaster are used for speed when
   they already exist. The session does not depend on them to survive.
4. Password and host-key prompts go through Taskmaster's askpass helper. They are
   not typed into the agent terminal and they are not parsed out of the
   protocol stream.
5. Treat status, hibernate, Review, and sidebar grouping as you would
   locally. A hosted session already running on that host can spawn
   children there. A lead that is still on the laptop cannot MCP-spawn
   onto the host. Start those workers from the palette or
   `taskmaster session spawn --host`.
6. Quit with `⌘Q` when you are done looking. Reopen Taskmaster and select the
   same sidebar row to attach again. The Engine keeps the session record;
   closing the window is not what ends the remote process.
7. Before you rely on a disconnect, open **Details** for that session. It
   says whether the remote process can survive disconnect. If it cannot,
   Taskmaster does not leave a `tmux` session for you to reattach. A Helper that
   fails verification fails closed with a structured error.
8. **Move Session to …** in the palette migrates the selected Claude
   session across hosts. That move is Claude-only, because resume has to
   be trustworthy. Other agents stay where you spawned them.
9. If you enrolled a `taskmaster-node`, handoff can move or fork a Codex or
   Claude thread without copying `auth.json`. Follow
   [Remote nodes](../remote-nodes.md). That path is not the SSH reconnect
   in steps 6 and 7.

## Expected result

- A remote row shows the host, and the terminal is the PTY on that machine.
- After you quit and reopen Taskmaster, the session is still in the sidebar and
  selecting it attaches again when the process is still alive.
- A host that cannot keep the process is reported as such, instead of
  looking like a multiplexer you can resume by hand.

## Pitfalls

- Tailscale addresses and MagicDNS names work only when OpenSSH can already
  resolve them. Taskmaster does not configure Tailscale.
- Do not rewrite a host's frozen `id` in `hosts.json` to "fix" a rename.
  Change the label. Sessions store the id.
- Do not copy provider `auth.json` onto the VPS as a shortcut. Node handoff
  exists so you do not have to.
- Two writing agents on one remote checkout still collide. Use a worktree
  there too ([Parallel agents in worktrees](parallel-worktrees.md)).
- There is no `tmux` fallback when the Helper is missing, corrupt, or the
  wrong build.

Host fields are in [Remote hosts](../remote-hosts.md). The day-to-day split
between the laptop and a big machine is also sketched in
[Fleet patterns](../fleet.md).
