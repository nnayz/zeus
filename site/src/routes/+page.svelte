<script lang="ts">
  import { base } from '$app/paths';
  import Home from '$lib/content/Home.md';
  import HeroImage from '$lib/components/HeroImage.svelte';
</script>

<svelte:head>
  <title>Zeus</title>
  <meta
    name="description"
    content="Agentic orchestrator for coding agents. Run Claude Code, Codex, and others in parallel. Closing the window never kills a session."
  />
</svelte:head>

<HeroImage
  src="{base}/preview.png"
  alt="Zeus desktop app: sidebar of agent sessions and a live terminal"
/>

<article class="markdown-content space-y-4"><Home /></article>
