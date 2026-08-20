<script lang="ts">
  import { base } from '$app/paths';
  import { GITHUB, SECURITY_EMAIL, latestRelease } from '$lib/releases';

  const latest = latestRelease();
</script>

# Security

## Current builds are not notarized

<p><a href="{base}/releases/{latest.version}/">v{latest.version}</a> is ad-hoc signed. Apple has not
notarized it. macOS Gatekeeper will warn or block a double-click. Follow the
<a href="{base}/install/">illustrated install guide</a> to create a one-app exception without
disabling Gatekeeper.</p>

The in-app updater requires Developer ID + notarization. It will not offer these builds. Later
signed releases will say so here and on the release page.

Download only from <a href="{GITHUB}/releases">GitHub Releases</a> under `nnayz/zeus`. Treat any
other binary as untrusted.

## Zeus is not a sandbox

Zeus launches shells, coding agents, MCP tools, and optional remote commands with your macOS user
privileges. It reduces orchestration mistakes. It does not inspect or approve each operation those
tools perform.

Worktrees avoid edit collisions. They are not a security boundary. For untrusted code, use a
dedicated OS account, VM, or container.

Remote sessions run under the SSH account you configure. Zeus does not add a separate authorization
layer.

## Data

No Zeus account, analytics, or telemetry. Session state and terminal replay live on your Mac under
Application Support. Logs can contain prompts, paths, and secrets printed by a process.

## Report a vulnerability

Email <a href="mailto:{SECURITY_EMAIL}?subject=Zeus%20security">{SECURITY_EMAIL}</a> with the subject
line `Zeus security`.

Include Zeus version, macOS version, a minimal reproduction, and expected impact.

Do not attach private terminal output, tokens, or personal paths unless they are required — and
mark that mail as confidential. Acknowledgement should arrive within seven days.

In scope: permission-boundary bypasses, unsafe update or IPC behavior, credential disclosure,
session isolation failures, unintended remote execution. A tool doing something you authorized is
not itself a Zeus vulnerability.
