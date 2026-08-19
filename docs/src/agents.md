# Agents and status

Every session is a PTY. The agent catalog is data: one JSON manifest per CLI,
describing how to spawn it, how to resume it, which keys approve or deny a
prompt, and the screen rules that decide whether it is working, waiting, or
done.

Zeus never installs an agent for you. Put the binary on `PATH`, sign in the
way that CLI expects, then launch it from Zeus.

## What you can launch

The launcher, sidebar **New Agent**, `⌘N`, `⌘T`, and the command palette all
read the same readiness catalog. First-class entries (when the CLI is
installed) include:

Claude Code, Codex, Cursor, Grok, OpenCode, Gemini, Aider, Amp, Copilot,
and the rest of the manifests shipped in the Engine.

A missing binary is not a crash. The row stays visible with an install hint
and, when the manifest has one, a setup URL.

`⌘T` always does something useful. If your saved default agent disappeared,
Zeus falls through to another installed first-class agent, then to a login
shell.

## Status you can leave the room on

Zeus reads the grid, the title, progress, hooks, and process facts. It
reduces them to a small vocabulary:

| State | Meaning |
|-------|---------|
| **Working** | The agent is doing something. In the workflow tree the mark wears a spinner. |
| **Needs input** | It is blocked on you: a permission, a question, a confirm. Destructive prompts get a sharper color. |
| **Done** | It finished work you have not opened yet. |
| **Idle** | Quiet, and you have already seen it. |
| **Hibernated** | Frozen to save memory. Opening the session wakes it in place. |
| **Ended** | The process exited. Resume if the agent supports it. |

Claude Code and Codex have the richest rules (working, blocked-permission,
blocked-question, idle, exited) plus resume. Other catalog agents get
whatever their manifest knows. If Zeus cannot name a blocker it will not
invent one. A lying spinner is worse than a quiet row.

`⌘⇧J` walks every session that needs input. Gentle status chimes (Settings →
General) are optional and stay quiet on mere token activity.

## Resume

Closing Zeus does not end the process. Closing a *session* might, after a
confirm if you left that setting on.

Agents that support resume can pick up a conversation after a clean exit or
after you reopen a still-running PTY. Codex and Claude Code are the ones to
trust here. Others may come back as a fresh CLI in the same folder.

Archiving (`⌘⇧W`) is for history, not resume.

## Hosted vs a bare shell

A **hosted** agent is one Zeus launched from a manifest. Those sessions get
Zeus MCP injected automatically (see [Orchestration](orchestration.md)).

A **plain terminal** (`⌥⌘T`) is your login shell. It still has a sidebar
row and running/exited status. It does not get agent MCP, resume, or a
branded mark.

## Marks in the tree

The workflow tree draws unbadged brand marks for Codex, Claude, Grok,
OpenCode, and Cursor. Other kinds use a terminal glyph. Working sessions
get a ring around the mark. Titles sit in a caption chip under the icon so
the rails never run through the name.

## Default agent

Settings → General → **Default agent** is what `⌘T` and Quick Open launch.
Pick the CLI you reach for first. Everything else stays one `⌘N` or `⌘K`
away.
