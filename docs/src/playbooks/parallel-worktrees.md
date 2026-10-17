# Parallel agents in worktrees

## When this is useful

Two agents are about to edit the same repository. A shared checkout means
they overwrite each other. Give each writer its own git worktree.

## Prerequisites

- The repository is open in Taskmaster (`⌘O` or `⌘P`).
- Each agent CLI is installed and signed in. Taskmaster does not install agents.
- `taskmaster doctor` succeeds if you will use the CLI. It checks that the Engine
  is up and that `claude` and `codex` are on `PATH`.

## Steps

1. Do not launch a second writing agent into the checkout the first one is
   already using.
2. Create the isolated checkout as part of the spawn.

   From a hosted lead, ask it to spawn the worker in a fresh worktree. Taskmaster
   MCP is already injected; the lead uses `spawn_agent` with `worktree: true`,
   or `create_worktree` and then a spawn into that path. See
   [Orchestration](../orchestration.md) if the lead is also delegating.

   From the CLI:

   ```sh
   taskmaster worktree create --repo ~/src/mldrills --branch spike-auth
   taskmaster session spawn claude-code --cwd ~/src/mldrills --worktree \
     --prompt "own the auth rewrite"
   ```

3. Rename the session (`⌘R`) after the bet (`auth-rewrite`, `spike-cache`),
   not `Claude Code 3`.
4. Open **Review** on that session. The diff is the session's cwd. If that
   cwd is the worktree, you are looking at that branch, not `main`.
5. Press `⌥⌘W` for the worktrees sheet: path, branch, owning session, dirty
   state, and whether Taskmaster thinks the tree is stale enough to remove.
   `taskmaster worktree list --repo ~/src/mldrills` prints the same catalog.
6. Keep the commits you want, then remove the tree. The sheet only offers
   cleanup for trees it believes are stale: old, merged, not dirty, and not
   `main`. It asks you to confirm. It will not force-remove a dirty tree.

   ```sh
   taskmaster worktree remove --repo ~/src/mldrills --path /path/to/worktree
   ```

## Expected result

- Each writing agent has its own working directory and branch on the same
  repository.
- The sidebar reads like a board of named bets.
- Review and **Files → Changed** follow the selected session's checkout.

## Pitfalls

- A worktree stops edit collisions. It is not a security boundary. The agent
  still runs as your user and still has your keys.
- Share one tree only when both sessions are supposed to touch the same
  files, such as a shell running tests under the implementer.
- MCP `spawn_agent` worktrees are local only. A laptop lead cannot create
  the remote worktree by passing a host. Start the remote session on that
  host first. See [Local or remote](local-or-remote.md).
- `taskmaster worktree remove --force` exists on the CLI. The app does not expose
  it. Do not delete a tree that still holds commits you have not moved.

More patterns for several spikes at once are in
[Fleet patterns](../fleet.md). The reference page is [Worktrees](../worktrees.md).
