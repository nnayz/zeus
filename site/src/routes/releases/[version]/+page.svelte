<script lang="ts">
  import ReleaseContent from '$lib/content/Release.md';
  import { formatDate } from '$lib/releases';

  let { data } = $props();
  const release = $derived(data.release);
</script>

<svelte:head>
  <title>v{release.version} · Zeus</title>
  <meta name="description" content={release.summary.join(' ')} />
</svelte:head>

<article class="markdown-content space-y-4">
  <header>
    <p class="text-[22px] leading-[30px] font-semibold">v{release.version}</p>
    <p class="mt-1 text-[13px] leading-5 text-muted">
      <time datetime={release.date}>{formatDate(release.date)}</time>
      {#if release.unsigned}
        <span class="tag ml-2 align-middle">unsigned</span>
      {/if}
    </p>
  </header>
  <ReleaseContent {release} />
</article>
