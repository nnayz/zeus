# Remote hosts

Add machines under **Settings → Remote**. Each host is an SSH destination
Taskmaster can spawn onto. The Mac stays the glass. The other computer owns the
PTY.

The catalog is stored per installation at:

```text
~/Library/Application Support/Taskmaster/hosts.json
```

A missing or empty file leaves Taskmaster in local-only mode. Prefer the Settings
form. The JSON is there if you want to copy a host between machines.

## Add a host

1. `⌘,` → **Remote** → add a host.
2. **Name** is the label in the palette (`New Codex on Forge`).
3. **SSH** is `you@forge`, a hostname, or an alias from `~/.ssh/config`.
4. **Default cwd** is where new sessions land (`~/code`).
5. Save. Open `⌘K` and type the host name.

| Field | Meaning |
|-------|---------|
| `id` | Stable value stored on sessions. Generated from the name, then frozen. |
| `name` | Label in pickers and badges |
| `ssh` | SSH destination or `~/.ssh/config` alias |
| `defaultCwd` | Default remote working directory for new sessions |

```json
{
  "hosts": [
    {
      "id": "forge",
      "name": "Forge",
      "ssh": "you@forge",
      "defaultCwd": "~/code"
    },
    {
      "id": "studio",
      "name": "Studio Mac",
      "ssh": "studio.local",
      "defaultCwd": "~/Developer"
    }
  ]
}
```

Tailscale IPv4 addresses and MagicDNS names work like any other SSH
destination when OpenSSH can resolve them. Taskmaster neither requires nor
configures Tailscale for remote holder sessions.

## How remote sessions work

SSH is only the authenticated, encrypted byte pipe. Taskmaster uploads a small
Helper (`taskmaster-remote`) the first time, then that Helper owns the Agent PTY
on the host. There is no requirement for remote `tmux`, Node.js, Python, or
a preinstalled Taskmaster service.

OpenSSH config and a short-lived ControlMaster are used for speed when they
already exist. Session survival does not depend on them.

macOS SSH password and host-key prompts go through Taskmaster's askpass helper.
They do not get parsed out of the protocol stream.

## Day to day

- **New agent on host** in the command palette (search the host name) spawns there.
- A hosted session *on that host* still gets MCP, so it can hire local
  children. A laptop lead cannot MCP-spawn onto the host yet. Use the
  palette or `taskmaster session spawn --host`. See
  [Orchestration](orchestration.md).
- **Move Session to …** in the palette migrates the selected Claude session
  across hosts (v1 is Claude-only, because resume has to be trustworthy).
- Status, hibernate, Review, and sidebar session grouping work the same as local.

If the Helper cannot be verified, Taskmaster fails closed with a structured
error. It will not fall back to `tmux`.

How to choose a host and get back to a session is the
[local or remote playbook](playbooks/local-or-remote.md). Protocol and
Holder internals are in the
[developer guide on GitHub](https://github.com/nnayz/taskmaster/blob/main/docs/dev/src/remote.md), not here.

Prefer a dedicated non-admin account and narrowly scoped credentials. See
the [security model](security-model.md). The transport design itself —
bootstrap, holder lifecycle, protocol, and performance gates — is documented
in the [developer guide on GitHub](dev/remote.html).

## First-party nodes

For provider accounts, fleet usage, and local↔cloud handoff on a VPS, enroll
a [`taskmaster-node`](remote-nodes.md) alongside the SSH host entry. The Settings
form has optional node endpoint, token file, and node id fields for that.
