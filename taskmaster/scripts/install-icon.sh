#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "Usage: scripts/install-icon.sh release|dev APP_PATH" >&2
    exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
assets_dir="$(cd "${script_dir}/../assets" && pwd)"
variant="$1"
app_path="$2"
case "${variant}" in
    release) icon_name="TaskmasterIcon"; icon_file="icon.icns" ;;
    dev) icon_name="TaskmasterDevIcon"; icon_file="dev-icon.icns" ;;
    *) echo "error: unknown icon variant: ${variant}" >&2; exit 2 ;;
esac

catalog="${assets_dir}/icon-assets/${variant}/Assets.car"
plist="${app_path}/Contents/Info.plist"
resources="${app_path}/Contents/Resources"
for source in "${catalog}" "${assets_dir}/${icon_file}" "${plist}"; do
    if [[ ! -f "${source}" ]]; then
        echo "error: missing icon packaging input: ${source}" >&2
        exit 1
    fi
done
plutil -lint "${plist}" >/dev/null
mkdir -p "${resources}"
cp "${catalog}" "${resources}/Assets.car"
cp "${assets_dir}/${icon_file}" "${resources}/${icon_file}"
# IconName selects the layered icon on current macOS; IconFile also keeps
# Finder and older macOS releases supplied with the matching raster family.
plutil -replace CFBundleIconName -string "${icon_name}" "${plist}"
plutil -replace CFBundleIconFile -string "${icon_file}" "${plist}"
cmp "${catalog}" "${resources}/Assets.car"
cmp "${assets_dir}/${icon_file}" "${resources}/${icon_file}"
echo "==> Installed ${icon_name} layered icon and ${icon_file} fallback"
