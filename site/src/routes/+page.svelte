<script lang="ts">
  import { base } from '$app/paths';
  import HeroImage from '$lib/components/HeroImage.svelte';
  import Mark from '$lib/components/Mark.svelte';
  import { GITHUB, latestRelease } from '$lib/releases';

  const latest = latestRelease();
</script>

<svelte:head>
  <title>Zeus</title>
  <meta
    name="description"
    content="The command center for coding agents. Real PTYs, live status, sessions that survive the window."
  />
</svelte:head>

<section class="hero">
  <div class="hero__lockup">
    <span class="hero__mark"><Mark /></span>
    <span class="hero__name">Zeus</span>
  </div>

  <h1>The command center for coding agents</h1>
  <p class="hero__lede">
    Native macOS app for Claude Code, Codex, Cursor, Grok, OpenCode, Gemini, and shells. Real PTYs,
    live status, and sessions that survive the window.
  </p>

  <div class="hero__actions">
    <a class="btn btn--primary" href={latest.dmg}>
      <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
        <path
          fill="currentColor"
          d="M12.54 11.07c-.1.24-.2.46-.32.68-.17.32-.37.63-.6.92-.32.4-.65.74-1 .74-.34 0-.45-.2-.85-.2s-.53.19-.86.2c-.37 0-.68-.35-1-.74a6.4 6.4 0 0 1-1.06-2.16c-1.12.07-2.18-.62-2.73-1.62-.9-1.64-.7-4.08.7-5.37.5-.47 1.16-.78 1.82-.78.36 0 .7.2 1.04.2.32 0 .73-.25 1.23-.25.45 0 .9.13 1.27.4-.03.02-1.2.7-1.19 2.1 0 1.67 1.45 2.23 1.48 2.24-.02.06-.23.8-.76 1.58Zm-1.7-8.3c.24-.3.41-.7.36-1.12-.35.02-.77.24-1.02.54-.22.25-.42.66-.37 1.05.38.03.78-.2 1.03-.47Z"
        />
      </svg>
      Download for Mac
    </a>
    <a class="btn btn--secondary" href={GITHUB} rel="external">
      <svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true">
        <path
          d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8"
        />
      </svg>
      View on GitHub
    </a>
  </div>

  <p class="hero__note">
    macOS 15+. Ad-hoc signed — right-click Open.
    <a href="{base}/install/">Install guide</a>
  </p>
</section>

<p class="features-label">Features</p>
<ul class="features">
  <li><strong>Live status:</strong> working, needs you, or done — without tailing every terminal</li>
  <li><strong>Persistent PTYs:</strong> close the window, the Engine keeps the session</li>
  <li><strong>Worktrees:</strong> concurrent writers get isolated Git checkouts</li>
  <li><strong>Remote hosts:</strong> SSH machines in the same sidebar</li>
  <li>
    <strong>Orchestration:</strong> a lead agent spawns children you can watch and accept
  </li>
  <li><strong>Review:</strong> read the diff in the window you used to watch the work</li>
  <li><strong>Native:</strong> macOS 15, GPUI — not a browser wrapper</li>
  <li><strong>iOS companion:</strong> check sessions from iPhone or iPad</li>
  <li><strong>Open source:</strong> Apache-2.0</li>
</ul>

<HeroImage
  src="{base}/preview.png"
  alt="Zeus desktop app: sidebar of agent sessions and a live terminal"
/>
