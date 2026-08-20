<script lang="ts">
  import { base } from '$app/paths';
  import { formatDate, type Release } from '$lib/releases';

  let { release }: { release: Release } = $props();
</script>

<div class="release-row">
  <a class="release-row__title hover:no-underline" href="{base}/releases/{release.version}/"
    >v{release.version}</a
  >
  <time class="release-row__date" datetime={release.date}>{formatDate(release.date)}</time>
  <p class="release-row__summary">
    {release.summary}
    {#if release.unsigned}
      <span class="tag ml-1 align-middle">unsigned</span>
    {/if}
  </p>
</div>
