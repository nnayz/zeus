<script lang="ts">
  import '../app.css';
  import Header from '$lib/components/Header.svelte';
  import Footer from '$lib/components/Footer.svelte';

  let { children } = $props();
</script>

<main class="max-w-[680px] mx-auto px-5 sm:px-8 md:px-12 py-14 md:py-32">
  <Header />
  {@render children()}
  <Footer />
</main>
