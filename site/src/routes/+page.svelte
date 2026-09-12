<script lang="ts">
  import { base } from '$app/paths';
  import Architecture from '$lib/components/Architecture.svelte';
  import InstallCommand from '$lib/components/InstallCommand.svelte';
  import TechnicalPanel from '$lib/components/TechnicalPanel.svelte';
  import { DOCS, latestRelease } from '$lib/releases';

  const latest = latestRelease();
</script>

<svelte:head>
  <title>Zeus</title>
  <meta
    name="description"
    content="Native command center for coding agents. Real PTYs, live status, sessions that survive the window."
  />
</svelte:head>

<section class="hero">
  <h1>Native command center for coding agents</h1>
  <div class="hero__actions">
    <a class="cta cta--primary" href={latest.dmg}>Download v{latest.version}</a>
    <a class="cta cta--secondary" href="{base}/install/">Install guide</a>
  </div>
  <p class="hero__claim">Real PTYs. Live status. Closing the window never kills a session.</p>
  <p class="hero__lede">
    Run Claude Code, Codex, Cursor, Grok, OpenCode, Gemini, and plain shells in parallel — locally
    or over SSH. Zeus is not an IDE and not a model. It is the place you watch a fleet and accept
    the work.
  </p>
</section>

<div class="product">
  <TechnicalPanel />
</div>

<section class="product section">
  <div class="section__head">
    <h2>The Engine holds the session</h2>
    <p>
      The desktop is a client of a local Engine. Remote hosts get a Helper that owns one PTY, not a
      second Zeus. Missing transport fails closed. There is no tmux fallback.
    </p>
  </div>
  <Architecture />
</section>

<section class="product section">
  <div class="section__head">
    <h2>Install</h2>
    <p>
      macOS 15 or newer. Universal build. v{latest.version} is ad-hoc signed, not notarized.
      Gatekeeper will warn. After dragging Zeus to Applications, right-click the app and choose
      Open.
    </p>
  </div>
  <InstallCommand command={latest.dmg} />
  <p class="copy" style="margin-top: 16px">
    Follow the <a href="{base}/install/">illustrated macOS install guide</a>
    for the one-time Gatekeeper step. Docs live at
    <a href={DOCS} rel="external">docs.zeus.nasrul.info</a>.
  </p>
</section>

<section class="product section">
  <div class="section__head">
    <h2>What’s new</h2>
  </div>
  {#each latest.summary as paragraph, index}
    <p class="copy">
      {#if index === 0}<a href="{base}/releases/{latest.version}/">v{latest.version}</a> —{' '}{/if}{paragraph}
    </p>
  {/each}
  <p class="copy copy--quiet">
    Do not treat v{latest.version} as an Apple-signed app. Read the
    <a href="{base}/security/">security notes</a> before installing.
  </p>
</section>
