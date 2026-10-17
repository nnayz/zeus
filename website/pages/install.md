# Install Taskmaster on macOS

Taskmaster requires macOS 15 or newer. The current DMG is a universal build for Apple silicon and Intel Macs.

> v{{version}} is ad-hoc signed, not Developer ID signed or notarized. That is why macOS shows a security warning. Continue only if you downloaded the DMG from the official nnayz/taskmaster GitHub release.

The annotated screens below illustrate macOS 15. Their spacing may differ slightly on your Mac, but the button and menu labels are the ones to look for.

## 1. Download the DMG

Download the latest universal installer: [Taskmaster v{{version}} (.dmg)]({{dmg}}).

Or copy the direct download URL:

```text
{{dmg}}
```

The file is named `taskmaster-{{version}}-universal.dmg`. Open it when the download finishes. A zip of the same build is at [taskmaster-{{version}}-universal.zip]({{zip}}).

## 2. Move Taskmaster to Applications

In the installer window, drag **Taskmaster** onto the **Applications** folder. Wait for the copy to finish, then eject the Taskmaster disk image.

<figure class="doc-shot">
<img src="/install/drag-to-applications.webp" alt="Illustration of the Taskmaster app being dragged onto the Applications folder" width="1360" height="907" loading="lazy" decoding="async">
<figcaption>Drag Taskmaster to Applications; do not run it from inside the disk image.</figcaption>
</figure>

## 3. Open Taskmaster from Finder

Open **Finder → Applications**. Control-click or right-click **Taskmaster**, then choose **Open**. If a confirmation dialog appears, choose **Open** again. macOS remembers this exception, so future launches work with a normal double-click.

<figure class="doc-shot">
<img src="/install/open-from-finder.webp" alt="Illustration of the Open command in Taskmaster's Finder context menu" width="1360" height="892" loading="lazy" decoding="async">
<figcaption>Use Open from Finder for the first launch. Launchpad does not show this menu.</figcaption>
</figure>

If Taskmaster opens, installation is complete. You do not need a Terminal command, and you should not disable Gatekeeper or remove quarantine attributes globally.

## If macOS still blocks Taskmaster

1. Double-click Taskmaster once. When the **“Taskmaster” Not Opened** warning appears, click **Done**. This records the blocked launch so macOS can offer a one-app exception.

<figure class="doc-shot">
<img src="/install/gatekeeper-warning.webp" alt="Illustration of the macOS Taskmaster Not Opened warning with the Done button circled" width="1360" height="907" loading="lazy" decoding="async">
<figcaption>The warning is expected for the current ad-hoc-signed build.</figcaption>
</figure>

2. Open **Apple menu → System Settings → Privacy & Security**. Scroll down to **Security**, find the message that Taskmaster was blocked, and click **Open Anyway**. The button is available for about an hour after the blocked launch.

<figure class="doc-shot">
<img src="/install/open-anyway.webp" alt="Illustration of the Open Anyway button for Taskmaster in macOS Privacy and Security settings" width="1360" height="750" loading="lazy" decoding="async">
<figcaption>Make an exception for Taskmaster only. Do not weaken Gatekeeper for every app.</figcaption>
</figure>

3. Authenticate with Touch ID or your Mac login password, then confirm **Open Anyway** once more. Taskmaster should launch and remain approved for later use.

If the Taskmaster message is missing, try opening the app once more and return to **Privacy & Security**. If macOS reports that the app is damaged, delete that copy and download the DMG again from the official release instead of bypassing the message.

## Why this warning appears

Apple has not notarized v{{version}}, so Gatekeeper cannot verify it with Apple. Taskmaster does not ask you to turn Gatekeeper off; the steps above create an exception for this copy of Taskmaster only. The in-app updater also remains disabled until a Developer ID signed and notarized release ships.

Read the [security notes](/security/) for the current trust model and download policy. The [GitHub release]({{github}}) is the only download you should trust.
