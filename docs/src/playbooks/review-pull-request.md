# Review changes and open a pull request

## When this is useful

An agent has edited a checkout and you want to read that diff, keep the
commits you mean to keep, and open or revisit the pull request without
losing the session.

## Prerequisites

- The session whose cwd you mean to review is selected. That cwd is often a
  worktree. See [Parallel agents in worktrees](parallel-worktrees.md).
- The inspector is open (`⌘⇧D`).
- To open a GitHub pull request, either the agent can push the way you
  usually do, or you can. Taskmaster does not create the pull request itself.

## Steps

1. Let the agent finish, or stop it on purpose. If the row needs you,
   resolve that first ([A session is waiting](needs-input.md)). **Done**
   means it finished work you have not opened yet.
2. Open the **Review** tab. You get the repository header, the branch, and
   the live diff for this session's cwd. If the session is in a worktree,
   this is that branch, not `main`.
3. Read the diff. **Files** opens on **Changed**, using the same staged,
   working-tree, rename, conflict, and untracked status as Review. `⌘⇧E`
   focuses that tree.
4. Stage and commit stay on the working tree. Commit only what belongs on
   this branch. `⌘J` opens an auxiliary shell under the session so git
   commands do not go into the agent's PTY.
5. Ask the agent to push and open the pull request, or do that from the
   auxiliary shell with your usual git and GitHub client. Say whether it
   may merge. The delegation example in
   [Delegate and follow child sessions](delegate.md) stops at a PR
   description and does not merge.
6. When you have a number or URL, open `⌘K` and run **Go to Pull Request…**.
   Enter `#123` or the GitHub URL, including when no agent printed the link.
   Opening a pull request head uses an isolated worktree.
7. **Branches…** in the palette compares or opens another ref. That is not
   an in-place checkout of the session you were reviewing.
8. **Details** can list pull requests for the session. It keeps Git to a
   one-shot summary that jumps to Review. It does not poll in the
   background or mutate the repository.

## Expected result

- The diff matches the selected session's checkout.
- After you or the agent opens the pull request, **Go to Pull Request…**
  loads that head in an isolated worktree.
- Quitting Taskmaster (`⌘Q`) does not discard the PTY. Closing the session can.

## Pitfalls

- Reviewing the wrong row reviews the wrong cwd. Check the session title
  and the Review repository header before you comment.
- Remote sessions are not browsed through the local filesystem. **Files →
  All files** is for the selected local checkout. Do not expect a remote
  tree to appear as local paths.
- **Go to Pull Request…** opens an existing pull request. It does not push
  the branch or click "Create" on GitHub for you.
- Archiving (`⌘⇧W`) keeps history. It does not merge, and it is not how you
  resume.
- The worktrees sheet will not throw away a dirty tree. Move or commit
  first if you still want those changes.

The panel tabs are documented in [The workbench](../workbench.md). Cleanup
rules are in [Worktrees](../worktrees.md).
