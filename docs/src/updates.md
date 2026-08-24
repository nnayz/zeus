# Updates

The updater is disabled for the current ad-hoc-signed release. It rejects
builds that are not Developer ID signed and notarized, including v0.2.0. Until
a qualifying release ships, download new versions manually from
[GitHub Releases](https://github.com/nnayz/zeus/releases).

Once signed releases are available, Zeus will use the flow below. Checks are
automatic; installing an update always waits for you.

## What you see

Checks run about 20 seconds after launch, then every 6 hours (toggleable in
**Settings → General → Updates**). Downloading and restarting are not automatic:

1. A background check finds a release → the sidebar footer shows an
   **Update to …** pill. Nothing else happens.
2. Click it → **Downloading…** with progress → the bundle is verified and
   staged.
3. Click **Restart to update…** → Zeus hands off to a helper and quits.

Zeus holds live agent sessions, so it never relaunches itself uninvited.
**⌘K → Check for Updates…** and the version row in the account popover both run
a manual check and report when you are already up to date.

## Trust model

There is no separate updater signing key. A downloaded bundle is accepted only
if all of the following hold:

1. `codesign --verify --deep --strict` passes.
2. Its **Team ID** and **bundle identifier** match the *running* app.
3. `spctl --assess --type execute` accepts it (notarization for a stapled
   bundle, without a network round trip).
4. Its short version string equals the version the feed promised.

At the feed layer, only strictly newer versions are offered (no downgrades), and
downloads use HTTPS to the releases host. The feed URL is stable:

```text
https://github.com/nnayz/zeus/releases/latest/download/appcast.json
```

## Local cache

Staging lives under:

```text
~/Library/Caches/zeus/updates/<version>/
```

If the app sits somewhere the user cannot write, the writability check fails
*before* the download starts.

Release packaging steps live in [`zeus/UPDATING.md`](https://github.com/nnayz/zeus/blob/main/zeus/UPDATING.md)
in the repository.
