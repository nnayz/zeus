<script lang="ts">
  import { base } from '$app/paths';
  import InstallCommand from '$lib/components/InstallCommand.svelte';
  import { formatDate } from '$lib/releases';

  let { data } = $props();
  const release = $derived(data.release);
</script>

<svelte:head>
  <title>v{release.version} · Zeus</title>
  <meta name="description" content={release.summary} />
</svelte:head>

<article class="space-y-4">
  <header>
    <p class="text-[22px] leading-[30px] font-semibold">v{release.version}</p>
    <p class="mt-1 text-[13px] leading-5 text-muted">
      <time datetime={release.date}>{formatDate(release.date)}</time>
      {#if release.unsigned}
        <span class="tag ml-2 align-middle">unsigned</span>
      {/if}
    </p>
  </header>

  <p>{release.summary}</p>

  <h2 class="font-medium pt-4">What's new</h2>
  <p>
    First public GitHub Release of the native Mac app. Universal binary. The Engine holds PTYs so
    quitting Zeus does not kill sessions.
  </p>

  <h2 class="font-medium pt-4">Install</h2>
  <p>
    Download the DMG, drag Zeus to Applications, then right-click → Open. macOS 15 or newer.
  </p>
  <InstallCommand command={release.dmg} />
  <p>
    <a href={release.github}>GitHub release</a>
    ·
    <a href={release.zip}>Update zip</a>
  </p>

  <h2 class="font-medium pt-4">Security</h2>
  <p>
    This build is ad-hoc signed. It is not Developer ID signed and not notarized. Gatekeeper will
    block a normal double-click; that is expected. The in-app updater will refuse it.
  </p>
  <p>
    Download only from <a href={release.github}>nnayz/zeus v{release.version}</a>. Full notes on
    <a href="{base}/security/">Security</a>.
  </p>
</article>
