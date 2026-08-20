<script lang="ts">
  import { base } from '$app/paths';
  import { formatDate, type Release } from '$lib/releases';

  let { release }: { release: Release } = $props();
</script>

<article class="release-row">
  <div class="release-row__meta">
    <time class="release-row__date" datetime={release.date}>{formatDate(release.date)}</time>
    <a class="release-row__version" href="{base}/releases/{release.version}/"
      >v{release.version}</a
    >
    {#if release.unsigned}
      <span class="tag">unsigned</span>
    {/if}
  </div>

  <div class="release-row__summary">
    {#each release.summary as paragraph}
      <p>{paragraph}</p>
    {/each}
  </div>
</article>
