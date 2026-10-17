# Agents and status

Every session is a PTY. The agent catalog is data: one JSON manifest per CLI,
describing how to spawn it, how to resume it, which keys approve or deny a
prompt, and the screen rules that decide whether it is working, waiting, or
done.

Taskmaster never installs an agent for you. Put the binary on `PATH`, sign in the
way that CLI expects, then launch it from Taskmaster.

## What you can launch

The launcher, sidebar **New Agent**, `⌘N`, `⌘T`, and the command palette all
read the same readiness catalog. First-class entries (when the CLI is
installed) include:

Claude Code, Codex, Cursor, Grok, OpenCode, Gemini, Aider, Amp, Copilot,
and the rest of the manifests shipped in the Engine.

A missing binary is not a crash. The row stays visible with an install hint
and, when the manifest has one, a setup URL.

`⌘T` always does something useful. If your saved default agent disappeared,
Taskmaster falls through to another installed first-class agent, then to a login
shell.

## Status you can leave the room on

Taskmaster reads the grid, the title, progress, hooks, and process facts. It
reduces them to a small vocabulary:

| State | Meaning |
|-------|---------|
| **Working** | The agent is doing something. |
| **Needs input** | It is blocked on you: a permission, a question, a confirm. Destructive prompts get a sharper color. |
| **Done** | It finished work you have not opened yet. |
| **Idle** | Quiet, and you have already seen it. |
| **Hibernated** | Frozen to save memory. Opening the session wakes it in place. |
| **Ended** | The process exited. Resume if the agent supports it. |

Claude Code and Codex have the richest rules (working, blocked-permission,
blocked-question, idle, exited) plus resume. Other catalog agents get
whatever their manifest knows. If Taskmaster cannot name a blocker it will not
invent one. A lying spinner is worse than a quiet row.

`⌘⇧J` walks every session that needs input. Gentle status chimes (Settings →
General) are optional and stay quiet on mere token activity.

## Resume

Closing Taskmaster does not end the process. Closing a *session* might, after a
confirm if you left that setting on.

Agents that support resume can pick up a conversation after a clean exit or
after you reopen a still-running PTY. Codex and Claude Code are the ones to
trust here. Others may come back as a fresh CLI in the same folder.

Archiving (`⌘⇧W`) is for history, not resume.

## Hosted vs a bare shell

A **hosted** agent is one Taskmaster launched from a manifest. Those sessions get
Taskmaster MCP injected automatically (see [Orchestration](orchestration.md)).

A **plain terminal** (`⌥⌘T`) is your login shell. It still has a sidebar
row and running/exited status. It does not get agent MCP, resume, or a
branded mark.

## Marks in the sidebar

Sidebar rows show the agent's brand mark and session status. Children nest
under their parent so the relationship remains visible. Select a row to open
its terminal.

## Default agent

Settings → General → **Default agent** is what `⌘T` and Quick Open launch.
Pick the CLI you reach for first. Everything else stays one `⌘N` or `⌘K`
away.
