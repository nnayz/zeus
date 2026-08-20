<script lang="ts">
  import { base } from '$app/paths';
  import InstallCommand from '$lib/components/InstallCommand.svelte';
  import { latestRelease } from '$lib/releases';

  const latest = latestRelease();
</script>

Agentic orchestrator for coding agents. Run Claude Code, Codex, Cursor, Grok, OpenCode, Gemini, and
plain shells in parallel, on Git worktrees or remote hosts.

Each session is a real PTY with a live status: working, needs you, or done. Closing the window never
kills an agent. A daemon restart brings the conversations back.

Zeus is not an IDE and not a model. It is the place you watch a fleet and accept the work.

## Install

macOS 15 or newer. Universal (Apple silicon and Intel).

Current builds are ad-hoc signed, not notarized. Gatekeeper will warn. After dragging Zeus to
Applications, right-click the app and choose Open.

<InstallCommand command={latest.dmg} />

Follow the <a href="{base}/install/">illustrated macOS install guide</a> for the one-time Gatekeeper
step. Or open the release itself on <a href={latest.github}>GitHub</a>. In-app updates stay off until
a Developer ID build ships.

## What's new

{#each latest.summary as paragraph, index}
  <p>{#if index === 0}<a href="{base}/releases/{latest.version}/">v{latest.version}</a> —{' '}{/if}{paragraph}</p>
{/each}

## Security

Do not treat v{latest.version} as an Apple-signed app. Read the <a href="{base}/security/">security
notes</a> before installing.
