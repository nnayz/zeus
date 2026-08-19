# Worktrees

Two agents editing the same checkout will overwrite each other. A git
worktree is a second working directory on a second branch, attached to the
same repository. Zeus treats that as the default way to run parallel work.

Worktrees are isolation from merge conflicts, not a security boundary.

## Create one

The everyday path is to ask a hosted agent. Zeus MCP is already injected, so
a lead can call `spawn_agent` with `worktree: true`, or `create_worktree`
first and then spawn into that path.

From the CLI:

```sh
zeus worktree create --repo ~/src/mldrills --branch spike-auth
zeus session spawn claude-code --cwd ~/src/mldrills --worktree --prompt "own the auth rewrite"
```

The new directory is a normal checkout. Commit, stash, or cherry-pick as
you would anywhere else.

## See what you have

`⌥⌘W` opens the worktrees sheet: path, branch, owning session, dirty or
not, and whether Zeus thinks the tree is stale enough to remove.

```sh
zeus worktree list --repo ~/src/mldrills
```

The inspector **Review** tab is bound to the session's cwd. If that cwd is
a worktree, you are reviewing that agent's branch, not `main`.

## Clean up

Treat each worktree as a real checkout. Commit or move the changes you want
**before** you delete it.

The sheet will only offer cleanup on trees it believes are stale (old,
merged, not dirty, not `main`). Confirmation is required. It will not
force-remove a dirty tree from the UI.

```sh
zeus worktree remove --repo ~/src/mldrills --path /path/to/worktree
```

`--force` exists on the CLI for when you know what you are doing. The app
does not expose that.

## A good default

Give every writing agent its own worktree. Share a tree only when two
sessions are *supposed* to touch the same files (a shell running tests
under the implementer, for example).

Name the session after the bet (`auth-rewrite`, `perf-spike`) so the
sidebar reads like a kanban, not a pile of "Claude Code".
