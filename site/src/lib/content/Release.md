<script lang="ts">
  import { base } from '$app/paths';
  import InstallCommand from '$lib/components/InstallCommand.svelte';
  import type { Release } from '$lib/releases';

  let { release }: { release: Release } = $props();
</script>

{#each release.summary as paragraph}
  <p>{paragraph}</p>
{/each}

## What's new

Start Claude Code, Codex, Cursor, Grok, OpenCode, Gemini, or a plain shell in a real PTY, locally or
over SSH. Zeus keeps each session alive in the Engine, shows when an agent is working or needs you,
and makes it easy to separate concurrent changes with Git worktrees.

## Install

Download the DMG, drag Zeus to Applications, then right-click → Open. macOS 15 or newer.

<InstallCommand command={release.dmg} />

<p>
  <a href="{base}/install/">Illustrated macOS install guide</a> ·
  <a href={release.github}>GitHub release</a> · <a href={release.zip}>Update zip</a>
</p>

## Security

This build is ad-hoc signed. It is not Developer ID signed and not notarized. Gatekeeper will block
a normal double-click; that is expected. The in-app updater will refuse it.

Download only from <a href={release.github}>nnayz/zeus v{release.version}</a>. Full notes on
<a href="{base}/security/">Security</a>.
