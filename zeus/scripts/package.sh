#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace_dir="$(cd "${script_dir}/.." && pwd)"
dist_dir="${ZEUS_DIST_DIR:-${workspace_dir}/dist}"
app_path="${dist_dir}/zeus.app"
# The updater compares against CARGO_PKG_VERSION, so artifact names have to come
# from the same place rather than a hand-passed number that can drift from it.
cargo_version="$(sed -n 's/^version = "\(.*\)"/\1/p' "${workspace_dir}/crates/zeus-app/Cargo.toml" | head -1)"
version="${ZEUS_VERSION:-${cargo_version:-0.2.0}}"
dmg_path="${dist_dir}/zeus-${version}-universal.dmg"
# Update artifact: a zip of the stapled bundle, which is what zeus's updater
# downloads. See crates/zeus-updater/src/install.rs.
zip_path="${dist_dir}/zeus-${version}-universal.zip"
entitlements="${workspace_dir}/assets/zeus.entitlements"
# NEVER default to /tmp/zeus-shared-target: that cache is shared with agent
# worktrees and cross-workspace fingerprint collisions produce Franken-builds
# (stale crates from other checkouts linked into the shipped app).
target_dir="${CARGO_TARGET_DIR:-${workspace_dir}/target}"
universal_dir="${target_dir}/universal-apple-darwin/release"
universal_binary="${universal_dir}/zeus"
universal_cli_binary="${universal_dir}/zeus-cli"
universal_mcp_binary="${universal_dir}/zeus-mcp"
universal_engine_binary="${universal_dir}/zeusd-rs"
universal_holder_binary="${universal_dir}/zeus-holder"
universal_askpass_binary="${universal_dir}/zeus-ssh-askpass"

# Toolchain location. The migration-era toolchain lived in /tmp, which macOS
# sweeps -- a reboot deleted it mid-project and releases could not be built at
# all until it was reinstalled. Prefer the persistent home install and fall back
# to /tmp only if that is where this machine still keeps it.
if [[ -x "${HOME}/.cargo/bin/cargo" ]]; then
    export CARGO_HOME="${CARGO_HOME:-${HOME}/.cargo}"
    export RUSTUP_HOME="${RUSTUP_HOME:-${HOME}/.rustup}"
else
    export CARGO_HOME="${CARGO_HOME:-/tmp/zeus-cargo-home}"
    export RUSTUP_HOME="${RUSTUP_HOME:-/tmp/zeus-rustup-home}"
fi
export PATH="${CARGO_HOME}/bin:${PATH}"
if ! command -v cargo >/dev/null 2>&1; then
    echo "error: no cargo on PATH (looked in ${CARGO_HOME}/bin)" >&2
    exit 1
fi
export CARGO_TARGET_DIR="${target_dir}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-15.0}"
export CLANG_MODULE_CACHE_PATH="${CLANG_MODULE_CACHE_PATH:-${target_dir}/clang-module-cache}"

if ! command -v cargo-packager >/dev/null 2>&1; then
    echo "error: cargo-packager is missing; install it with 'cargo install cargo-packager --locked'" >&2
    exit 1
fi

cd "${workspace_dir}"

echo "==> Building zeus for Apple silicon"
cargo build --release --package zeus-app --bin zeus --package zeus-cli --bin zeus-cli \
    --package zeus-mcp --bin zeus-mcp --target aarch64-apple-darwin

echo "==> Building zeus for Intel"
cargo build --release --package zeus-app --bin zeus --package zeus-cli --bin zeus-cli \
    --package zeus-mcp --bin zeus-mcp --target x86_64-apple-darwin

echo "==> Creating universal executable"
mkdir -p "${universal_dir}" "${dist_dir}"
lipo -create \
    "${target_dir}/aarch64-apple-darwin/release/zeus" \
    "${target_dir}/x86_64-apple-darwin/release/zeus" \
    -output "${universal_binary}"
lipo "${universal_binary}" -verify_arch arm64 x86_64
lipo -create \
    "${target_dir}/aarch64-apple-darwin/release/zeus-cli" \
    "${target_dir}/x86_64-apple-darwin/release/zeus-cli" \
    -output "${universal_cli_binary}"
lipo "${universal_cli_binary}" -verify_arch arm64 x86_64
lipo -create \
    "${target_dir}/aarch64-apple-darwin/release/zeus-mcp" \
    "${target_dir}/x86_64-apple-darwin/release/zeus-mcp" \
    -output "${universal_mcp_binary}"
lipo "${universal_mcp_binary}" -verify_arch arm64 x86_64

echo "==> Assembling ${app_path} with cargo-packager"
cargo packager \
    --release \
    --packages zeus-app \
    --formats app \
    --target universal-apple-darwin \
    --binaries-dir "${universal_dir}" \
    --out-dir "${dist_dir}"

app_bin_dir="${app_path}/Contents/Resources/bin"
echo "==> Bundling CLI and lightweight MCP proxy into Resources/bin"
mkdir -p "${app_bin_dir}"
cp "${universal_cli_binary}" "${app_bin_dir}/zeus"
cp "${universal_mcp_binary}" "${app_bin_dir}/zeus-mcp"

# The Rust Engine is the authoritative daemon launched by zeus. The remote
# Helper catalog below is consumed by this executable directly.
echo "==> Building the authoritative Rust Engine (universal)"
cargo build --release --package zeus-engine --bin zeusd-rs --bin zeus-holder \
    --bin zeus-ssh-askpass \
    --target aarch64-apple-darwin
cargo build --release --package zeus-engine --bin zeusd-rs --bin zeus-holder \
    --bin zeus-ssh-askpass \
    --target x86_64-apple-darwin
lipo -create \
    "${target_dir}/aarch64-apple-darwin/release/zeusd-rs" \
    "${target_dir}/x86_64-apple-darwin/release/zeusd-rs" \
    -output "${universal_engine_binary}"
lipo -create \
    "${target_dir}/aarch64-apple-darwin/release/zeus-holder" \
    "${target_dir}/x86_64-apple-darwin/release/zeus-holder" \
    -output "${universal_holder_binary}"
lipo -create \
    "${target_dir}/aarch64-apple-darwin/release/zeus-ssh-askpass" \
    "${target_dir}/x86_64-apple-darwin/release/zeus-ssh-askpass" \
    -output "${universal_askpass_binary}"
lipo "${universal_engine_binary}" -verify_arch arm64 x86_64
lipo "${universal_holder_binary}" -verify_arch arm64 x86_64
lipo "${universal_askpass_binary}" -verify_arch arm64 x86_64
cp "${universal_engine_binary}" "${app_bin_dir}/zeusd-rs"
cp "${universal_holder_binary}" "${app_bin_dir}/zeus-holder"
cp "${universal_askpass_binary}" "${app_bin_dir}/zeus-ssh-askpass"

# The default SSH transport bootstraps one exact Rust Helper artifact selected
# by remote OS/architecture. This build is independent of all daemon products
# above and emits a versioned manifest verified again before upload.
echo "==> Building three-platform Rust remote Helper catalog"
remote_helpers_dir="${app_bin_dir}/remote-helpers"
rm -rf "${remote_helpers_dir}"
ZEUS_SIGN_IDENTITY="${ZEUS_SIGN_IDENTITY:-}" "${script_dir}/build-remote-helpers.sh" "${remote_helpers_dir}"

# Rust-owned Agent catalog used by local and remote session orchestration.
rm -rf "${app_bin_dir}/manifests"
cp -R "${workspace_dir}/crates/zeus-engine/manifests" "${app_bin_dir}/manifests"
# Count, not just presence. A catalog that is merely SMALLER never errors at
# runtime: each missing manifest silently downgrades that agent to a bare login
# shell, which looks like a working session. That shipped once. The source
# directory is the reference, so any shrink between it and the bundle is a
# packaging bug worth failing the release over.
source_manifests="$(find "${workspace_dir}/crates/zeus-engine/manifests" -name '*.json' -type f | wc -l | tr -d ' ')"
bundled_manifests="$(find "${app_bin_dir}/manifests" -name '*.json' -type f | wc -l | tr -d ' ')"
if [[ ! -f "${app_bin_dir}/manifests/codex.json" || "${bundled_manifests}" != "${source_manifests}" || "${bundled_manifests}" -lt 20 ]]; then
    echo "error: bundled Agent catalog is incomplete: ${bundled_manifests} manifest(s) bundled, ${source_manifests} in source (expected at least 20)" >&2
    exit 1
fi
echo "==> Bundled ${bundled_manifests} Agent manifests"

# Inside-out signing: sign the nested daemon binaries FIRST (their own hardened
# runtime + timestamp), then the app LAST WITHOUT --deep. A --deep sign would
# re-stamp the nested executables with the app's identifier and can fail
# notarization; nested Mach-O must be signed independently.
# Auto-detect a Developer ID when none is given, exactly as release.sh does.
# TCC keys privacy grants (Documents/Desktop access prompts) to the signing
# identity: ad-hoc changes every rebuild, so each dev install used to re-ask —
# a stable Developer ID makes one "Allow" stick across every future install.
if [[ -z "${ZEUS_SIGN_IDENTITY:-}" ]]; then
    ZEUS_SIGN_IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null \
        | grep "Developer ID Application" | head -1 \
        | sed -E 's/.*"(.*)".*/\1/' || true)"
fi
sign_id="${ZEUS_SIGN_IDENTITY:--}"
ts_flag=(--timestamp)
[[ "${sign_id}" == "-" ]] && ts_flag=(--timestamp=none) && echo "==> No signing identity found; ad-hoc signature"
[[ "${sign_id}" != "-" ]] && echo "==> Signing with: ${sign_id}"
echo "==> Signing nested executables"
codesign --force --options runtime "${ts_flag[@]}" --sign "${sign_id}" "${app_bin_dir}/zeus"
codesign --force --options runtime "${ts_flag[@]}" --sign "${sign_id}" "${app_bin_dir}/zeus-mcp"
codesign --force --options runtime "${ts_flag[@]}" --sign "${sign_id}" "${app_bin_dir}/zeusd-rs"
codesign --force --options runtime "${ts_flag[@]}" --sign "${sign_id}" "${app_bin_dir}/zeus-holder"
codesign --force --options runtime "${ts_flag[@]}" --sign "${sign_id}" "${app_bin_dir}/zeus-ssh-askpass"
# The Apple remote Helper is deliberately NOT signed here. Signing rewrites the
# Mach-O, and its length and digest are already recorded in the catalog manifest
# that the Engine verifies before upload; signing after the fact invalidates the
# manifest and the Engine rejects the entire catalog, disabling every remote
# host. build-remote-helpers.sh signs it before measuring instead.
echo "==> Signing ${app_path}"
codesign --force --options runtime "${ts_flag[@]}" \
    --entitlements "${entitlements}" \
    --identifier com.zeus.zeus \
    --sign "${sign_id}" \
    "${app_path}"

codesign --verify --deep --strict "${app_path}"

notary_profile="${APPLE_NOTARIZATION_KEYCHAIN_PROFILE:-${APPLE_KEYCHAIN_PROFILE:-${NOTARY_PROFILE:-}}}"
notary_apple_id="${APPLE_NOTARIZATION_APPLE_ID:-${APPLE_ID:-}}"
notary_password="${APPLE_NOTARIZATION_PASSWORD:-${APPLE_PASSWORD:-}}"
notary_team_id="${APPLE_NOTARIZATION_TEAM_ID:-${APPLE_TEAM_ID:-}}"
notary_requested=0
if [[ -n "${notary_profile}" || -n "${notary_apple_id}" || -n "${notary_password}" || -n "${notary_team_id}" ]]; then
    notary_requested=1
fi

# One place that knows how to talk to notarytool, called for the app zip and
# again for the DMG.
submit_for_notarization() {
    local artifact="$1"
    if [[ -n "${notary_profile}" ]]; then
        xcrun notarytool submit "${artifact}" --keychain-profile "${notary_profile}" --wait
    elif [[ -n "${notary_apple_id}" && -n "${notary_password}" && -n "${notary_team_id}" ]]; then
        xcrun notarytool submit "${artifact}" \
            --apple-id "${notary_apple_id}" \
            --password "${notary_password}" \
            --team-id "${notary_team_id}" \
            --wait
    else
        echo "error: set a keychain profile or all APPLE_NOTARIZATION_{APPLE_ID,PASSWORD,TEAM_ID} values" >&2
        exit 1
    fi
}

if [[ "${notary_requested}" == "1" ]]; then
    if [[ -z "${ZEUS_SIGN_IDENTITY:-}" ]]; then
        echo "error: notarization requires ZEUS_SIGN_IDENTITY" >&2
        exit 1
    fi

    # The .app is notarized and stapled BEFORE the DMG is built, so both the
    # DMG's copy and the update zip carry their own ticket. A ticket stapled
    # only to the DMG would leave the extracted bundle needing an online
    # Gatekeeper check, and the updater verifies downloads offline.
    echo "==> Notarizing ${app_path}"
    notarization_zip="$(mktemp -d "${TMPDIR:-/tmp}/zeus-notarize.XXXXXX")/zeus.zip"
    ditto -c -k --keepParent "${app_path}" "${notarization_zip}"
    submit_for_notarization "${notarization_zip}"
    rm -rf "$(dirname "${notarization_zip}")"
    xcrun stapler staple "${app_path}"
    xcrun stapler validate "${app_path}"
fi

if [[ "${ZEUS_CREATE_DMG:-0}" == "1" ]]; then
    echo "==> Creating ${dmg_path}"
    dmg_stage="$(mktemp -d "${TMPDIR:-/tmp}/zeus-dmg.XXXXXX")"
    cleanup_dmg_stage() {
        rm -rf "${dmg_stage}"
    }
    trap cleanup_dmg_stage EXIT
    ditto "${app_path}" "${dmg_stage}/zeus.app"
    ln -s /Applications "${dmg_stage}/Applications"
    rm -f "${dmg_path}"
    hdiutil create -quiet -volname "zeus" -srcfolder "${dmg_stage}" -ov -format UDZO "${dmg_path}"
    if [[ -n "${ZEUS_SIGN_IDENTITY:-}" ]]; then
        codesign --force --timestamp --sign "${ZEUS_SIGN_IDENTITY}" "${dmg_path}"
    fi
fi

if [[ "${ZEUS_CREATE_DMG:-0}" == "1" && "${notary_requested}" == "1" ]]; then
    echo "==> Notarizing ${dmg_path}"
    submit_for_notarization "${dmg_path}"
    xcrun stapler staple "${dmg_path}"
    xcrun stapler validate "${dmg_path}"
fi

if [[ "${ZEUS_CREATE_ZIP:-0}" == "1" || "${notary_requested}" == "1" ]]; then
    echo "==> Creating ${zip_path}"
    rm -f "${zip_path}"
    # --keepParent puts zeus.app at the archive root, which is the layout the
    # updater's unpack step requires.
    ditto -c -k --keepParent "${app_path}" "${zip_path}"
fi

bundle_size="$(du -sh "${app_path}" | awk '{print $1}')"
echo "Built ${app_path} (${bundle_size})"
if [[ "${ZEUS_CREATE_DMG:-0}" == "1" ]]; then
    echo "Built ${dmg_path}"
fi
if [[ -f "${zip_path}" ]]; then
    echo "Built ${zip_path}"
fi
