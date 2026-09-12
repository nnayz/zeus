<script lang="ts">
  import { base } from '$app/paths';
  import { GITHUB, SECURITY_EMAIL } from '$lib/releases';
</script>

<footer class="site-footer">
  <p>Apache-2.0. <a href="{GITHUB}/blob/main/LICENSE">License</a>.</p>
  <p>
    <a href="{base}/security/">Security</a>
    ·
    <a href="mailto:{SECURITY_EMAIL}">{SECURITY_EMAIL}</a>
  </p>
  <p>Built by <a href="https://nasrul.info">Nasrul</a>.</p>
</footer>
