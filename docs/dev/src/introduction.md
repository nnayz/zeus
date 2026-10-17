# Developer Guide

This book is for people building and maintaining Taskmaster. It is not the product
manual.

The [User Guide](../../src/introduction.md) covers install, the workbench,
playbooks, remote operation, Companion setup, and support. Use that book
when the question is how to run Taskmaster. Use this one when the question is how
the repository is built, tested, or changed.

## What is authoritative

Architecture and protocol decisions stay in the repository files that already
own them. This guide links to those files. It does not restate them as a
second copy.

| Question | Read |
|----------|------|
| Remote PTY transport, SSH, Holders | [`taskmaster/REMOTE_PORT.md`](https://github.com/nnayz/taskmaster/blob/main/taskmaster/REMOTE_PORT.md) |
| Packaging a release | [`taskmaster/PACKAGING.md`](https://github.com/nnayz/taskmaster/blob/main/taskmaster/PACKAGING.md) |
| Updater implementation | [`taskmaster/UPDATING.md`](https://github.com/nnayz/taskmaster/blob/main/taskmaster/UPDATING.md) |
| Companion HTTP contract and trust model | [Companion architecture](companion-architecture.md) in this book |
| Companion gateway behavior | [`taskmaster/COMPANION_GATEWAY.md`](https://github.com/nnayz/taskmaster/blob/main/taskmaster/COMPANION_GATEWAY.md) |
| Companion terminal commands | [`taskmaster/COMPANION_TERMINAL.md`](https://github.com/nnayz/taskmaster/blob/main/taskmaster/COMPANION_TERMINAL.md) |

If this guide and `taskmaster/REMOTE_PORT.md` disagree about remote sessions, the
port document wins. Historical port notes are marked on
[Historical documents](historical.md). They are not current requirements.

The full index, including which audience each existing document serves, is
[Engineering index](engineering-index.md).

## Start here

1. [Build and run](getting-started.md) from a fresh checkout through the
   required checks.
2. [Workspace map](workspace.md) for which crate owns what.
3. [Documentation](documentation.md) before adding a page.
