#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace_dir="$(cd "${script_dir}/.." && pwd)"
target_dir="${CARGO_TARGET_DIR:-${workspace_dir}/target}"
profile="debug"
settings_preview=""
cargo_args=()

usage() {
    cat <<'USAGE'
Usage: scripts/dev.sh [--release] [--settings TAB] [-- CARGO_BUILD_ARGS...]

Build and launch an unmistakable development copy of zeus.

Options:
  --release       Build with Cargo's release profile.
  --settings TAB  Open Settings on general, terminal, resources, or remote.
  -h, --help      Show this help.

Arguments after -- are passed to cargo build. Options that change Cargo's
target directory, target triple, or profile are not supported; set
CARGO_TARGET_DIR or use --release instead.
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)
            profile="release"
            cargo_args+=("$1")
            shift
            ;;
        --settings)
            if [[ $# -lt 2 ]]; then
                echo "error: --settings requires a tab" >&2
                usage >&2
                exit 2
            fi
            settings_preview="$2"
            shift 2
            ;;
        --settings=*)
            settings_preview="${1#*=}"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            cargo_args+=("$@")
            break
            ;;
        *)
            echo "error: unknown option: $1 (put cargo build arguments after --)" >&2
            usage >&2
            exit 2
            ;;
    esac
done

case "${settings_preview}" in
    ""|general|terminal|resources|remote) ;;
    *)
        echo "error: unknown Settings tab: ${settings_preview}" >&2
        exit 2
        ;;
esac

if (( ${#cargo_args[@]} > 0 )); then
    for argument in "${cargo_args[@]}"; do
        case "${argument}" in
            --target|--target=*|--target-dir|--target-dir=*|--profile|--profile=*)
                echo "error: ${argument} changes where the app binary is written" >&2
                exit 2
                ;;
        esac
    done
fi

branch="$(git -C "${workspace_dir}" symbolic-ref --quiet --short HEAD || true)"
branch="${branch:-detached}"
short_sha="$(git -C "${workspace_dir}" rev-parse --short=8 HEAD)"
dirty=""
if ! git -C "${workspace_dir}" diff --quiet --ignore-submodules -- \
    || ! git -C "${workspace_dir}" diff --cached --quiet --ignore-submodules --; then
    dirty="+dirty"
fi
build_label="${branch}@${short_sha}${dirty}"
# Stable on purpose: macOS TCC keys Files & Folders grants to the signed app
# identity. The build label/window chrome still distinguishes commits without
# invalidating Desktop/Documents access on every rebuild.
bundle_id="com.zeus.zeus.dev.local"
display_name="zeus dev ${short_sha}"

mkdir -p "${target_dir}"

cd "${workspace_dir}"
echo "==> Building ${display_name} (${profile})"
# zeus-app does not pull the Engine or automation helpers into target/<profile>/.
# Build the complete local process family so every process that touches a
# selected folder can run from the signed app bundle. Launching zeusd-rs or
# zeus-holder loose from target/ loses the app's macOS folder authorization.
if (( ${#cargo_args[@]} > 0 )); then
    cargo build --package zeus-app --bin zeus --package zeus-engine --bin zeusd-rs \
        --bin zeus-holder --bin zeus-ssh-askpass \
        --package zeus-cli --bin zeus-cli --package zeus-mcp --bin zeus-mcp \
        --package zeus-remote --bin zeus-remote \
        "${cargo_args[@]}"
else
    cargo build --package zeus-app --bin zeus --package zeus-engine --bin zeusd-rs \
        --bin zeus-holder --bin zeus-ssh-askpass \
        --package zeus-cli --bin zeus-cli --package zeus-mcp --bin zeus-mcp \
        --package zeus-remote --bin zeus-remote
fi

binary="${target_dir}/${profile}/zeus"
if [[ ! -x "${binary}" ]]; then
    echo "error: cargo did not produce ${binary}" >&2
    exit 1
fi

engine_bin="${target_dir}/${profile}/zeusd-rs"
if [[ ! -x "${engine_bin}" ]]; then
    echo "error: cargo did not produce ${engine_bin}" >&2
    exit 1
fi
cli_bin="${target_dir}/${profile}/zeus-cli"
mcp_bin="${target_dir}/${profile}/zeus-mcp"
holder_bin="${target_dir}/${profile}/zeus-holder"
askpass_bin="${target_dir}/${profile}/zeus-ssh-askpass"
remote_bin="${target_dir}/${profile}/zeus-remote"
for helper in "${cli_bin}" "${mcp_bin}" "${holder_bin}" "${askpass_bin}" "${remote_bin}"; do
    if [[ ! -x "${helper}" ]]; then
        echo "error: cargo did not produce ${helper}" >&2
        exit 1
    fi
done

# Every invocation gets a fresh bundle. Replacing a bundle beneath a still-
# running process invalidates its code signature, which is especially easy to
# do when judging two builds side by side.
bundle_root="$(mktemp -d "${target_dir}/zeus-dev-${short_sha}.XXXXXX")"
app_path="${bundle_root}/${display_name}.app"
contents="${app_path}/Contents"
app_bin_dir="${contents}/Resources/bin"
mkdir -p "${contents}/MacOS" "${app_bin_dir}"
cp "${binary}" "${contents}/MacOS/zeus"
cp "${workspace_dir}/assets/dev-icon.icns" "${contents}/Resources/dev-icon.icns"
cp "${engine_bin}" "${app_bin_dir}/zeusd-rs"
cp "${cli_bin}" "${app_bin_dir}/zeus"
cp "${mcp_bin}" "${app_bin_dir}/zeus-mcp"
cp "${holder_bin}" "${app_bin_dir}/zeus-holder"
cp "${askpass_bin}" "${app_bin_dir}/zeus-ssh-askpass"
cp "${remote_bin}" "${app_bin_dir}/zeus-remote"
cp -R "${workspace_dir}/crates/zeus-engine/manifests" "${app_bin_dir}/manifests"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "${workspace_dir}/crates/zeus-app/Cargo.toml" | head -1)"
cat > "${contents}/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleDisplayName</key><string>${display_name}</string>
    <key>CFBundleExecutable</key><string>zeus</string>
    <key>CFBundleIconFile</key><string>dev-icon.icns</string>
    <key>CFBundleIdentifier</key><string>${bundle_id}</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleName</key><string>${display_name}</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>${version}</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>15.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
</dict>
</plist>
PLIST

# A stable development certificate keeps TCC grants across rebuilds. Prefer an
# explicit identity, then the repository's optional "Zeus Dev" certificate;
# ad-hoc remains a zero-setup fallback for contributors who do not need
# protected Desktop/Documents folders.
dev_sign_id="${ZEUS_DEV_SIGN_IDENTITY:-}"
if [[ -z "${dev_sign_id}" ]] && security find-identity -v -p codesigning 2>/dev/null | grep -q '"Zeus Dev"'; then
    dev_sign_id="Zeus Dev"
fi
if [[ -z "${dev_sign_id}" ]]; then
    dev_sign_id="-"
    echo "==> Warning: ad-hoc signing; Files & Folders grants may not survive rebuilds"
    echo "    Run ../scripts/make-dev-cert.sh once, or set ZEUS_DEV_SIGN_IDENTITY."
else
    echo "==> Signing development bundle with: ${dev_sign_id}"
fi

# Sign nested executables first so the bundle seals a coherent helper family.
for helper in \
    "${app_bin_dir}/zeus" \
    "${app_bin_dir}/zeus-mcp" \
    "${app_bin_dir}/zeusd-rs" \
    "${app_bin_dir}/zeus-holder" \
    "${app_bin_dir}/zeus-ssh-askpass" \
    "${app_bin_dir}/zeus-remote"
do
    codesign --force --sign "${dev_sign_id}" "${helper}"
done
codesign --force --sign "${dev_sign_id}" \
    --entitlements "${workspace_dir}/assets/zeus.entitlements" \
    --identifier "${bundle_id}" \
    "${app_path}"
codesign --verify --deep --strict "${app_path}"

launch_environment=("ZEUS_DEV=1" "ZEUS_DEV_BUILD=${build_label}")
if [[ -n "${settings_preview}" ]]; then
    launch_environment+=("ZEUS_SETTINGS_PREVIEW=${settings_preview}")
fi

# The bundled Engine above is authoritative. An explicitly exported
# ZEUSD_PATH remains a developer override, but this script no longer points a
# signed app at loose target/ helpers that lack its folder authorization.

echo "==> Launching ${display_name} (${build_label})"
echo "    ${app_path}"
if [[ -n "${ZEUSD_PATH:-}" ]]; then
    echo "    engine: ${ZEUSD_PATH}"
fi
for item in "${launch_environment[@]}"; do
    if [[ "${item}" == ZEUSD_PATH=* ]]; then
        echo "    engine: ${item#ZEUSD_PATH=}"
    fi
done
exec env \
    -u ZEUS_SOCKET \
    -u ZEUS_SESSION_ID \
    -u ZEUS_CLI \
    -u NO_COLOR \
    "${launch_environment[@]}" \
    "${contents}/MacOS/zeus"
