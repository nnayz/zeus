# Remote transport

Operators add hosts and reconnect from the User Guide:
[Remote hosts](../../src/remote-hosts.md) and the
[local or remote playbook](../../src/playbooks/local-or-remote.md). This page is
only the contributor entry.

The authoritative design is
[`taskmaster/REMOTE_PORT.md`](https://github.com/nnayz/taskmaster/blob/main/taskmaster/REMOTE_PORT.md).
Read it before changing remote session behavior, SSH handling, PTYs, holders,
terminal state, or packaging of the Helper. If code and that document
disagree, stop and resolve the mismatch. Do not pick one silently, and do
not copy the document into this book.

Current constraints, all defined there:

- The bootstrapped Remote PTY Holder is the only remote session transport.
  The old `ssh -t` + `tmux` path is gone. Do not reintroduce it as a feature
  flag, a migration, or a fallback.
- SSH is the authenticated byte pipe (`ssh -T`). The Helper owns the agent
  PTY.
- A missing or unverified Helper fails closed (`remote_transport_unavailable`
  when the packaged transport cannot be used).
- One Holder process and one Unix socket per session. No multi-session
  supervisor in the current baseline.
- `taskmaster-remote` stays a Helper. It does not become a second Engine.
- `taskmaster-node` is optional enhanced mode. It is not on the default SSH
  bootstrap path. Operator setup stays in the User Guide
  [Remote nodes](../../src/remote-nodes.md).

`taskmaster/PORT.md` describes an earlier engine port and is
[historical](historical.md). It is not the remote transport spec.
