<script lang="ts">
  import '../app.css';
  import Header from '$lib/components/Header.svelte';
  import Footer from '$lib/components/Footer.svelte';
  import type { LayoutProps } from './$types';

  let { children, data }: LayoutProps = $props();
</script>

<div class="page">
  <Header githubStars={data.githubStars} />
  <div class="page-body">
    {@render children()}
  </div>
  <Footer />
</div>
