<script lang="ts">
  import ReleaseList from '$lib/components/ReleaseList.svelte';
</script>

<svelte:head>
  <title>Releases · Zeus</title>
  <meta name="description" content="Zeus release notes and download archive." />
</svelte:head>

<article class="space-y-4">
  <h1 class="font-medium">Releases</h1>
  <p class="text-muted">
    Each row is a GitHub Release. Unsigned cuts are marked. Prefer the latest version.
  </p>
  <ReleaseList />
</article>
