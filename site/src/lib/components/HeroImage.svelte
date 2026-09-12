<script lang="ts">
  let { src, alt }: { src: string; alt: string } = $props();
</script>

<div class="overflow-hidden rounded-[10px] mb-8 bg-surface border border-border">
  <img {src} {alt} class="block w-full h-auto" />
</div>
