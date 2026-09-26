<script lang="ts">
  let { src, alt }: { src: string; alt: string } = $props();
</script>

<div class="hero-shot">
  <img {src} {alt} class="block w-full h-auto" />
</div>
