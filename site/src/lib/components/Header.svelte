<script lang="ts">
  import { base } from '$app/paths';
  import { GITHUB, DOCS } from '$lib/releases';

  let { title = 'Zeus' }: { title?: string } = $props();
</script>

<header class="flex items-center justify-between mb-6">
  <a href="{base}/" class="text-[15px] font-semibold leading-5 text-[var(--color-text)] hover:no-underline"
    >{title}</a
  >
  <nav class="flex items-center gap-3 sm:gap-4 text-[15px] font-medium leading-5">
    <a href="{base}/install/">Install</a>
    <a href="{base}/releases/">Releases</a>
    <a href="{base}/security/" class="hidden sm:inline">Security</a>
    <a href={DOCS} rel="external">Docs</a>
    <a href={GITHUB}>GitHub</a>
  </nav>
</header>
