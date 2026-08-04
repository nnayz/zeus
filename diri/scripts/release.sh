#!/bin/bash
# Build, sign, notarize, and publish a diri release.
#
# Usage: diri/scripts/release.sh <version>        e.g. diri/scripts/release.sh 0.2.0
#
# Env overrides:
#   DIRI_SIGN_IDENTITY  "Developer ID Application: ..." (default: auto-detected)
#   NOTARY_PROFILE      notarytool keychain profile (default: dirijor-notary)
#   RELEASES_DIR        path to the dirijor-releases repo (default: ../../dirijor-releases)
#   SKIP_GATES=1        skip cargo test/clippy (for re-running a failed publish)
#   SKIP_PERF_GATE=1   skip packaged app memory/idle-CPU probe
#
# This publishes TWO artifacts per release, both notarized and stapled:
#   diri-<version>-universal.dmg  what people download by hand
#   diri-<version>-universal.zip  what the in-app updater fetches
# and rewrites diri/appcast.json, the feed the updater reads. See
# diri/UPDATING.md for the trust model and one-time setup.
set -euo pipefail

if [ $# -lt 1 ]; then
    echo "usage: diri/scripts/release.sh <version>   (e.g. 0.2.0)" >&2
    exit 2
fi
VERSION="$1"
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: version '$VERSION' is not X.Y.Z" >&2
    exit 2
fi

WORKSPACE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$(cd "$WORKSPACE/.." && pwd)"
cd "$WORKSPACE"

DIST="$WORKSPACE/dist"
APP="$DIST/diri.app"
DMG="$DIST/diri-$VERSION-universal.dmg"
ZIP="$DIST/diri-$VERSION-universal.zip"
MANIFEST="$WORKSPACE/crates/diri-app/Cargo.toml"

NOTARY_PROFILE="${NOTARY_PROFILE:-dirijor-notary}"
RELEASES_DIR="${RELEASES_DIR:-$ROOT/../dirijor-releases}"
RELEASES_HOST="https://dirijor-releases.crisemcr.workers.dev"
# Everything diri publishes lives under /diri/ so it never collides with the
# Swift app's DMGs and appcast.xml in the same bucket.
PUBLIC_DIR="$RELEASES_DIR/public/diri"
FEED="$PUBLIC_DIR/appcast.json"
MINIMUM_SYSTEM="15.0"
# Old builds stay downloadable but the repo should not grow without bound.
KEEP_RELEASES=5

# See package.sh: prefer the persistent home toolchain over the /tmp one, which
# macOS sweeps out from under us.
if [ -x "$HOME/.cargo/bin/cargo" ]; then
    export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
    export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
else
    export CARGO_HOME="${CARGO_HOME:-/tmp/diri-cargo-home}"
    export RUSTUP_HOME="${RUSTUP_HOME:-/tmp/diri-rustup-home}"
fi
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$WORKSPACE/target}"

if [ -z "${DIRI_SIGN_IDENTITY:-}" ]; then
    # `|| true`: grep exits 1 with no match, which pipefail would turn into an
    # abort before the friendly error below.
    DIRI_SIGN_IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null \
        | grep "Developer ID Application" | head -1 \
        | sed -E 's/.*"(.*)".*/\1/' || true)"
fi
if [ -z "${DIRI_SIGN_IDENTITY:-}" ]; then
    cat >&2 <<EOF
error: no "Developer ID Application" signing identity found.

  Create one in Xcode → Settings → Accounts → Manage Certificates → +
  → "Developer ID Application", then re-run. Or set DIRI_SIGN_IDENTITY.
  See diri/UPDATING.md.
EOF
    exit 1
fi

echo "==> Releasing diri $VERSION"
echo "    Sign identity : $DIRI_SIGN_IDENTITY"
echo "    Notary profile: $NOTARY_PROFILE"
echo "    Releases dir  : $PUBLIC_DIR"

if [ ! -d "$RELEASES_DIR" ]; then
    echo "error: releases repo not found at $RELEASES_DIR (set RELEASES_DIR)" >&2
    exit 1
fi

# ----------------------------------------------------------------------------
# 1. Version bump
# ----------------------------------------------------------------------------
# The updater compares against CARGO_PKG_VERSION, so the manifest is the single
# source of truth for what version this build claims to be.
CURRENT="$(sed -n 's/^version = "\(.*\)"/\1/p' "$MANIFEST" | head -1)"
if [ "$CURRENT" != "$VERSION" ]; then
    if [ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no)" ]; then
        echo "error: uncommitted changes; commit them before bumping to $VERSION" >&2
        exit 1
    fi
    echo "==> Bumping diri-app $CURRENT -> $VERSION"
    sed -i '' -E "1,/^version = /s/^version = \".*\"/version = \"$VERSION\"/" "$MANIFEST"
    cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace >/dev/null
    git -C "$ROOT" add "$MANIFEST" "$WORKSPACE/Cargo.lock"
    git -C "$ROOT" commit -m "diri: release $VERSION" >/dev/null
    echo "    committed the bump"
fi

# ----------------------------------------------------------------------------
# 2. Gates
# ----------------------------------------------------------------------------
if [ "${SKIP_GATES:-0}" != "1" ]; then
    echo "==> Running release gates"
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
fi

# ----------------------------------------------------------------------------
# 3. Build, sign, notarize, staple (app first, then DMG — see package.sh)
# ----------------------------------------------------------------------------
# Agent worktrees are told to share this target dir (see the warning in
# package.sh). A build from a worktree whose sources differ writes artifacts
# that cargo then considers fresh here -- a cross-workspace fingerprint
# collision -- so a stale crate from another checkout can be linked in silently.
# It has happened: an x86_64 diri-term with a NEWER mtime than its source but
# missing a method the source had. A compile error is the lucky outcome; the
# unlucky one is a signed, notarized build shipping someone else's code. Third-
# party deps cannot collide this way (they are immutable at a given version), so
# only first-party crates are purged and the expensive GPUI build stays cached.
echo "==> Purging first-party build artifacts (cross-worktree cache safety)"
FIRST_PARTY=(-p diri-app -p diri-client -p diri-proto -p diri-term -p diri-ui -p diri-updater)
for release_target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo clean "${FIRST_PARTY[@]}" --release --target "$release_target"
done

echo "==> Packaging (notarization can take a few minutes)"
DIRI_VERSION="$VERSION" \
DIRI_SIGN_IDENTITY="$DIRI_SIGN_IDENTITY" \
DIRI_CREATE_DMG=1 \
DIRI_CREATE_ZIP=1 \
APPLE_NOTARIZATION_KEYCHAIN_PROFILE="$NOTARY_PROFILE" \
    "$WORKSPACE/scripts/package.sh"

for artifact in "$APP" "$DMG" "$ZIP"; do
    if [ ! -e "$artifact" ]; then
        echo "error: packaging did not produce $artifact" >&2
        exit 1
    fi
done

# The updater refuses a download whose ticket does not validate offline, so
# check that here rather than discovering it from a user's failed update.
echo "==> Verifying the stapled bundle the updater will install"
xcrun stapler validate "$APP"
spctl --assess --type execute -vv "$APP"

# The regression probe must run against this exact signed/notarized bundle.
# It owns and terminates only the two Diri processes it launches.
if [ "${SKIP_PERF_GATE:-0}" != "1" ]; then
    echo "==> Running packaged memory/idle-CPU gate"
    "$WORKSPACE/scripts/perf-gate.sh" --app "$APP" --scenario all
fi

# ----------------------------------------------------------------------------
# 4. Publish the artifacts
# ----------------------------------------------------------------------------
mkdir -p "$PUBLIC_DIR"
cp "$DMG" "$ZIP" "$PUBLIC_DIR/"

# Point the site's download button at this release. diri bundles the daemon and
# is meant to replace Dirijor.app, so a diri release takes over the button that
# the Swift release.sh used to claim for its own DMG. SKIP_INDEX=1 leaves it on
# whatever it currently serves.
INDEX="$RELEASES_DIR/public/index.html"
if [ "${SKIP_INDEX:-0}" != "1" ] && [ -f "$INDEX" ]; then
    if grep -q 'id="download"' "$INDEX"; then
        sed -i '' -E \
            "s|(id=\"download\"[^>]*href=\")[^\"]*(\")|\1diri/diri-$VERSION-universal.dmg\2|" \
            "$INDEX"
        echo "==> Download button now serves diri-$VERSION-universal.dmg"
        echo "    (the page still reads \"Dirijor\" — rename it there when you're ready)"
    else
        echo "warning: no id=\"download\" anchor in $INDEX; left the button alone" >&2
    fi
fi

NOTES_FILE="$PUBLIC_DIR/diri-$VERSION.html"
NOTES_URL=""
if [ -f "$NOTES_FILE" ]; then
    NOTES_URL="$RELEASES_HOST/diri/diri-$VERSION.html"
else
    echo "    (no release notes at $NOTES_FILE — the update UI will show just the version)"
fi

SIZE="$(stat -f%z "$ZIP")"
SHA256="$(shasum -a 256 "$ZIP" | awk '{print $1}')"
PUBLISHED="$(date -u +%Y-%m-%d)"

echo "==> Updating $FEED"
VERSION="$VERSION" \
URL="$RELEASES_HOST/diri/diri-$VERSION-universal.zip" \
SIZE="$SIZE" SHA256="$SHA256" PUBLISHED="$PUBLISHED" \
NOTES_URL="$NOTES_URL" MINIMUM_SYSTEM="$MINIMUM_SYSTEM" \
FEED="$FEED" KEEP_RELEASES="$KEEP_RELEASES" \
python3 - <<'PY'
import json, os, pathlib

feed_path = pathlib.Path(os.environ["FEED"])
feed = {"feed_version": 1, "releases": []}
if feed_path.exists():
    feed = json.loads(feed_path.read_text())
    feed.setdefault("feed_version", 1)
    feed.setdefault("releases", [])

version = os.environ["VERSION"]
entry = {
    "version": version,
    "url": os.environ["URL"],
    "size": int(os.environ["SIZE"]),
    "sha256": os.environ["SHA256"],
    "minimum_system_version": os.environ["MINIMUM_SYSTEM"],
    "published": os.environ["PUBLISHED"],
}
if os.environ.get("NOTES_URL"):
    entry["notes_url"] = os.environ["NOTES_URL"]

# Re-releasing a version replaces its row rather than adding a second one the
# client would have to disambiguate.
releases = [r for r in feed["releases"] if r.get("version") != version]
releases.append(entry)


def sort_key(release):
    parts = (release.get("version") or "0").split(".")
    return tuple(int(part) if part.isdigit() else 0 for part in (parts + ["0", "0", "0"])[:3])


releases.sort(key=sort_key, reverse=True)
feed["releases"] = releases[: int(os.environ["KEEP_RELEASES"])]
feed_path.write_text(json.dumps(feed, indent=2) + "\n")
print(f"    {len(feed['releases'])} release(s) in the feed, newest {feed['releases'][0]['version']}")
PY

# Drop artifacts that fell off the end of the feed.
python3 - "$PUBLIC_DIR" "$FEED" <<'PY'
import json, pathlib, sys

public = pathlib.Path(sys.argv[1])
kept = {r["version"] for r in json.loads(pathlib.Path(sys.argv[2]).read_text())["releases"]}
for artifact in list(public.glob("diri-*-universal.dmg")) + list(public.glob("diri-*-universal.zip")):
    version = artifact.name.removeprefix("diri-").removesuffix("-universal.dmg").removesuffix("-universal.zip")
    if version not in kept:
        artifact.unlink()
        print(f"    pruned {artifact.name}")
PY

# ----------------------------------------------------------------------------
# 5. Commit + deploy
# ----------------------------------------------------------------------------
echo "==> Committing release in $RELEASES_DIR"
git -C "$RELEASES_DIR" add -A
git -C "$RELEASES_DIR" commit -m "diri release $VERSION" || echo "    (nothing to commit)"

echo "==> Deploying to Cloudflare (wrangler)"
( cd "$RELEASES_DIR" && pnpm dlx wrangler deploy 2>&1 | tail -3 )

cat <<EOF

============================================================
  diri $VERSION released
============================================================
  DMG        : $PUBLIC_DIR/diri-$VERSION-universal.dmg
  Update zip : $PUBLIC_DIR/diri-$VERSION-universal.zip
  sha256     : $SHA256
  Feed       : $RELEASES_HOST/diri/appcast.json

  Next steps:
    1. Push the releases repo: git -C "$RELEASES_DIR" push
    2. Push + tag the source:  git -C "$ROOT" push && git tag diri-v$VERSION && git push origin diri-v$VERSION
    3. Confirm an old build updates itself (diri/UPDATING.md → "Verifying a release")
============================================================
EOF
