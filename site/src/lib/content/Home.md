<script lang="ts">
  import { base } from '$app/paths';
  import InstallCommand from '$lib/components/InstallCommand.svelte';
  import { latestRelease } from '$lib/releases';

  const latest = latestRelease();
</script>

## Install

macOS 15+. Universal. Ad-hoc signed — drag to Applications, then right-click Open.

<InstallCommand command={latest.dmg} />

<p>
  <a href="{base}/install/">Install guide</a>
  ·
  <a href={latest.github}>GitHub</a>
  ·
  <a href="{base}/security/">Security</a>
</p>

## What's new

<p>
  <a href="{base}/releases/{latest.version}/">v{latest.version}</a> — Git workspace review, restored
  workspaces, Zeus Dark themes.
</p>
