<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchGithubStars, formatStarCount } from '$lib/github';

  let { initial = null }: { initial?: number | null } = $props();
  let live = $state<number | null>(null);
  let stars = $derived(live ?? initial);

  onMount(() => {
    const controller = new AbortController();
    void fetchGithubStars(fetch, { signal: controller.signal, cache: 'no-store' }).then((count) => {
      if (count != null) live = count;
    });
    return () => controller.abort();
  });
</script>

{#if stars != null}
  <span class="inline-flex items-center gap-0.5 text-[12px] font-normal leading-4 tabular-nums text-white/40">
    <svg class="size-2.5" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      <path
        d="M8 .25a.75.75 0 0 1 .673.418l1.882 3.815 4.21.612a.75.75 0 0 1 .416 1.279l-3.046 2.97.719 4.192a.751.751 0 0 1-1.088.791L8 12.347l-3.766 1.98a.75.75 0 0 1-1.088-.79l.72-4.194L.818 6.374a.75.75 0 0 1 .416-1.28l4.21-.611L7.327.668A.75.75 0 0 1 8 .25Z"
      />
    </svg>
    {formatStarCount(stars)}<span class="sr-only">{' '}stars</span>
  </span>
{/if}
