<script lang="ts">
  import { base } from '$app/paths';
  import { GITHUB, SECURITY_EMAIL, latestRelease } from '$lib/releases';

  const latest = latestRelease();
</script>

<svelte:head>
  <title>Security · Zeus</title>
  <meta
    name="description"
    content="Unsigned-build warning, trust boundaries, and how to report a Zeus vulnerability."
  />
</svelte:head>

<article class="space-y-4">
  <h1 class="font-medium">Security</h1>

  <h2 class="font-medium pt-4">Current builds are not notarized</h2>
  <p>
    <a href="{base}/releases/{latest.version}/">v{latest.version}</a> is ad-hoc signed. Apple has not
    notarized it. macOS Gatekeeper will warn or block a double-click. Right-click the app → Open.
  </p>
  <p>
    The in-app updater requires Developer ID + notarization. It will not offer these builds. Later
    signed releases will say so here and on the release page.
  </p>
  <p>
    Download only from <a href="{GITHUB}/releases">GitHub Releases</a> under
    <code class="inline-code">nnayz/zeus</code>. Treat any other binary as untrusted.
  </p>

  <h2 class="font-medium pt-4">Zeus is not a sandbox</h2>
  <p>
    Zeus launches shells, coding agents, MCP tools, and optional remote commands with your macOS
    user privileges. It reduces orchestration mistakes. It does not inspect or approve each
    operation those tools perform.
  </p>
  <p>
    Worktrees avoid edit collisions. They are not a security boundary. For untrusted code, use a
    dedicated OS account, VM, or container.
  </p>
  <p>
    Remote sessions run under the SSH account you configure. Zeus does not add a separate
    authorization layer.
  </p>

  <h2 class="font-medium pt-4">Data</h2>
  <p>
    No Zeus account, analytics, or telemetry. Session state and terminal replay live on your Mac
    under Application Support. Logs can contain prompts, paths, and secrets printed by a process.
  </p>

  <h2 class="font-medium pt-4">Report a vulnerability</h2>
  <p>
    Email <a href="mailto:{SECURITY_EMAIL}?subject=Zeus%20security">{SECURITY_EMAIL}</a> with the
    subject line <code class="inline-code">Zeus security</code>.
  </p>
  <p>Include Zeus version, macOS version, a minimal reproduction, and expected impact.</p>
  <p>
    Do not attach private terminal output, tokens, or personal paths unless they are required — and
    mark that mail as confidential. Acknowledgement should arrive within seven days.
  </p>
  <p>
    In scope: permission-boundary bypasses, unsafe update or IPC behavior, credential disclosure,
    session isolation failures, unintended remote execution. A tool doing something you authorized
    is not itself a Zeus vulnerability.
  </p>
</article>
