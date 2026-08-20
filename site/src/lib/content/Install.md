<script lang="ts">
  import { base } from '$app/paths';
  import InstallCommand from '$lib/components/InstallCommand.svelte';
  import { latestRelease } from '$lib/releases';

  const latest = latestRelease();
</script>

# Install Zeus on macOS

Zeus requires macOS 15 or newer. The current DMG is a universal build for Apple silicon and Intel
Macs.

<div class="install-note" role="note" aria-label="Unsigned build warning">
  <strong>Unsigned build.</strong>
  <p>
    v{latest.version} is ad-hoc signed, not Developer ID signed or notarized. That is why macOS
    shows a security warning. Continue only if you downloaded the DMG from the official
    <a href={latest.github}>nnayz/zeus GitHub release</a>.
  </p>
</div>

The annotated screens below illustrate macOS 15. Their spacing may differ slightly on your Mac,
but the button and menu labels are the ones to look for.

## 1. Download the DMG

Download the latest universal installer:

<p>
  <a href={latest.dmg}>Download Zeus v{latest.version} (.dmg)</a>
</p>

Or copy the direct download URL:

<InstallCommand command={latest.dmg} />

The file is named `zeus-{latest.version}-universal.dmg`. Open it when the download finishes.

## 2. Move Zeus to Applications

In the installer window, drag **Zeus** onto the **Applications** folder. Wait for the copy to
finish, then eject the Zeus disk image.

<figure class="install-shot">
  <img
    src="{base}/install/drag-to-applications.webp"
    alt="Illustration of the Zeus app being dragged onto the Applications folder"
    width="1360"
    height="907"
    loading="lazy"
    decoding="async"
  />
  <figcaption>Drag Zeus to Applications; do not run it from inside the disk image.</figcaption>
</figure>

## 3. Open Zeus from Finder

Open **Finder → Applications**. Control-click or right-click **Zeus**, then choose **Open**. If a
confirmation dialog appears, choose **Open** again. macOS remembers this exception, so future
launches work with a normal double-click.

<figure class="install-shot">
  <img
    src="{base}/install/open-from-finder.webp"
    alt="Illustration of the Open command in Zeus's Finder context menu"
    width="1360"
    height="892"
    loading="lazy"
    decoding="async"
  />
  <figcaption>Use Open from Finder for the first launch. Launchpad does not show this menu.</figcaption>
</figure>

If Zeus opens, installation is complete. You do not need a Terminal command, and you should not
disable Gatekeeper or remove quarantine attributes globally.

## If macOS still blocks Zeus

1. Double-click Zeus once. When the **“Zeus” Not Opened** warning appears, click **Done**. This
   records the blocked launch so macOS can offer a one-app exception.

<figure class="install-shot">
  <img
    src="{base}/install/gatekeeper-warning.webp"
    alt="Illustration of the macOS Zeus Not Opened warning with the Done button circled"
    width="1360"
    height="907"
    loading="lazy"
    decoding="async"
  />
  <figcaption>The warning is expected for the current ad-hoc-signed build.</figcaption>
</figure>

2. Open **Apple menu → System Settings → Privacy & Security**. Scroll down to **Security**, find
   the message that Zeus was blocked, and click **Open Anyway**. The button is available for about
   an hour after the blocked launch.

<figure class="install-shot">
  <img
    src="{base}/install/open-anyway.webp"
    alt="Illustration of the Open Anyway button for Zeus in macOS Privacy and Security settings"
    width="1360"
    height="750"
    loading="lazy"
    decoding="async"
  />
  <figcaption>Make an exception for Zeus only—do not weaken Gatekeeper for every app.</figcaption>
</figure>

3. Authenticate with Touch ID or your Mac login password, then confirm **Open Anyway** once more.
   Zeus should launch and remain approved for later use.

If the Zeus message is missing, try opening the app once more and return to **Privacy & Security**.
If macOS reports that the app is damaged, delete that copy and download the DMG again from the
official release instead of bypassing the message.

## Why this warning appears

Apple has not notarized v{latest.version}, so Gatekeeper cannot verify it with Apple. Zeus does not
ask you to turn Gatekeeper off; the steps above create an exception for this copy of Zeus only.
The in-app updater also remains disabled until a Developer ID signed and notarized release ships.

Read the <a href="{base}/security/">security notes</a> for the current trust model and download
policy.
