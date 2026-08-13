# Remote hosts

Add, edit, or remove execution hosts from **Settings → Remote**. The catalog is
stored per installation in:

```text
~/Library/Application Support/Zeus/hosts.json
```

A missing or empty file leaves Zeus in local-only mode.

## Catalog format

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

| Field | Meaning |
|-------|---------|
| `id` | Stable value stored on sessions |
| `name` | Label in pickers and badges (falls back to `id`) |
| `ssh` | SSH destination or an alias from `~/.ssh/config` |
| `defaultCwd` | Default remote working directory for new sessions |

Tailscale IPv4 addresses and MagicDNS names work like any other SSH destination
when OpenSSH can resolve them. Zeus neither requires nor configures Tailscale
for remote holder sessions.

## How remote sessions work

Remote agent sessions use SSH only as an authenticated, encrypted byte
transport. Zeus bootstraps a small remote Helper (`zeus-remote`) that owns the
Agent PTY on the host. There is no requirement for remote `tmux`, Node.js, or a
preinstalled Zeus service.

Prefer a dedicated non-admin account and narrowly scoped credentials. See the
[security model](security-model.md).

## First-party nodes

For provider accounts, fleet usage, and local↔cloud handoff on a VPS, enroll a
[`zeus-node`](remote-nodes.md) alongside the SSH host entry.
