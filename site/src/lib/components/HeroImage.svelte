<script lang="ts">
  let { src, alt }: { src: string; alt: string } = $props();
</script>

<div class="overflow-hidden rounded-lg mb-8 bg-surface ring-1 ring-border">
  <img {src} {alt} class="block w-full h-auto" />
</div>
