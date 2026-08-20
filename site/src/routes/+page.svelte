<script lang="ts">
  import { base } from '$app/paths';
  import HeroImage from '$lib/components/HeroImage.svelte';
  import InstallCommand from '$lib/components/InstallCommand.svelte';
  import { latestRelease } from '$lib/releases';

  const latest = latestRelease();
</script>

<svelte:head>
  <title>Zeus</title>
  <meta
    name="description"
    content="Agentic orchestrator for coding agents. Run Claude Code, Codex, and others in parallel. Closing the window never kills a session."
  />
</svelte:head>

<HeroImage
  src="{base}/preview.png"
  alt="Zeus desktop app: sidebar of agent sessions and a live terminal"
/>

<article class="space-y-4">
  <p>
    Agentic orchestrator for coding agents. Run Claude Code, Codex, Cursor, Grok, OpenCode,
    Gemini, and plain shells in parallel, on git worktrees or remote hosts.
  </p>
  <p>
    Each session is a real PTY with a live status: working, needs you, or done. Closing the window
    never kills an agent. A daemon restart brings the conversations back.
  </p>
  <p>Zeus is not an IDE and not a model. It is the place you watch a fleet and accept the work.</p>

  <h2 class="font-medium pt-4">Install</h2>
  <p>macOS 15 or newer. Universal (Apple silicon and Intel).</p>
  <p>
    Current builds are ad-hoc signed, not notarized. Gatekeeper will warn. After dragging Zeus to
    Applications, right-click the app and choose Open.
  </p>
  <InstallCommand command={latest.dmg} />
  <p>
    Or grab the DMG from <a href={latest.github}>GitHub Releases</a>. In-app updates stay off until
    a Developer ID build ships.
  </p>

  <h2 class="font-medium pt-4">What's new</h2>
  <p>
    <a href="{base}/releases/{latest.version}/">v{latest.version}</a>
    — {latest.summary}
  </p>

  <h2 class="font-medium pt-4">Security</h2>
  <p>
    Do not treat v{latest.version} as an Apple-signed app. Read the
    <a href="{base}/security/">security notes</a> before installing.
  </p>
</article>
