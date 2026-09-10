<script lang="ts">
  import '../app.css';
  import Header from '$lib/components/Header.svelte';
  import Footer from '$lib/components/Footer.svelte';
  import type { LayoutProps } from './$types';

  let { children, data }: LayoutProps = $props();
</script>

<main class="max-w-[680px] mx-auto px-5 sm:px-8 md:px-12 py-14 md:py-32">
  <Header githubStars={data.githubStars} />
  {@render children()}
  <Footer />
</main>
