# homebrew-zeus

Homebrew tap for [Zeus](https://github.com/nnayz/zeus), a native macOS
orchestrator for coding agents.

Maintained by Nasrul Huda ([@nnayz](https://github.com/nnayz),
[hi@nasrul.info](mailto:hi@nasrul.info)).

```sh
brew tap nnayz/zeus https://github.com/nnayz/zeus.git
brew install --cask nnayz/zeus/zeus
```

This tap is part of the [`nnayz/zeus`](https://github.com/nnayz/zeus) monorepo,
not a separate `nnayz/homebrew-zeus` repository. The explicit URL tells Homebrew
to use the monorepo for the `nnayz/zeus` tap. The repository-root `Casks` link
exposes this directory in Homebrew's required tap layout.

After tapping, the short cask name also works:

```sh
brew install --cask zeus
```

## Updating

The cask is maintained by the release tooling in the main Zeus repository.
`zeus/scripts/release.sh` publishes the immutable DMG, then updates the version
and SHA-256 here, commits and pushes that monorepo change, and reads the remote
cask back for verification. The cask should not be edited by hand during a
release.

Zeus updates itself, so the cask declares `auto_updates true`: Homebrew performs
the initial installation and does not overwrite a newer in-app update.

## Uninstalling

```sh
brew uninstall --cask zeus
```

`brew uninstall --zap --cask zeus` removes Zeus preferences and caches. It
deliberately preserves `~/Library/Application Support/Zeus`, which contains
session records, host configuration, and holder state. Removing the client must
not destroy sessions that a later installation can reattach.
