<script lang="ts">
  import ReleaseRow from '$lib/components/ReleaseRow.svelte';
  import { releases } from '$lib/releases';
</script>

{#if releases.length === 0}
  <p>No releases yet.</p>
  <p class="text-muted">The first release will appear here.</p>
{:else}
  <div>
    {#each releases as release (release.version)}
      <ReleaseRow {release} />
    {/each}
  </div>
{/if}
