# Packaging and releases

How a user receives an update is the User Guide [Updates](../../src/updates.md)
page. How a release is cut, signed, and fed to the updater is not.

| Topic | Authoritative file |
|-------|--------------------|
| Universal app bundle, Helper artifacts, `package.sh` | [`taskmaster/PACKAGING.md`](https://github.com/nnayz/taskmaster/blob/main/taskmaster/PACKAGING.md) |
| Updater trust model, feed, and why there is no Sparkle key | [`taskmaster/UPDATING.md`](https://github.com/nnayz/taskmaster/blob/main/taskmaster/UPDATING.md) |
| Local packaging and install commands also named from the root README | `taskmaster/scripts/package.sh`, `taskmaster/scripts/install-local.sh` |

Read those files before changing the bundle layout, the Helper catalog, or
the update feed. This page does not restate the signing rules. A second
summary would drift.

Current product constraint, which the User Guide already tells people: the
shipping DMG may be ad-hoc signed. The updater rejects builds that are not
Developer ID signed and notarized. Do not document Homebrew or in-app update
as available until that is true in the User Guide and the root README.

Release-publishing guard tests run from `./scripts/check.sh`:

```sh
bash taskmaster/scripts/test-publish-github-release.sh
bash taskmaster/scripts/test-publish-homebrew-cask.sh
```

Icon artwork and regeneration are in
[`taskmaster/assets/icon-assets/README.md`](https://github.com/nnayz/taskmaster/blob/main/taskmaster/assets/icon-assets/README.md).
