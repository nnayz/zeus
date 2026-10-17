#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace_dir="$(cd "${script_dir}/.." && pwd)"
assets_dir="${workspace_dir}/assets"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/taskmaster-icon.XXXXXX")"

cleanup() {
    if [[ "${TASKMASTER_KEEP_ICON_WORK_DIR:-0}" == "1" ]]; then
        echo "Kept icon workspace at ${work_dir}" >&2
        return
    fi
    rm -rf "${work_dir}"
}
trap cleanup EXIT

# The editable Icon Composer document is the source for both the layered
# catalog and the raster fallback. Only regeneration needs Xcode 26 or newer;
# ordinary builds consume the checked-in compiled resources.
source_icon="${assets_dir}/TaskmasterIcon.icon"
dev_icon="${work_dir}/TaskmasterDevIcon.icon"
if [[ ! -f "${source_icon}/icon.json" || ! -f "${assets_dir}/DevIndicator.svg" ]]; then
    echo "error: missing Taskmaster Icon Composer artwork" >&2
    exit 1
fi

# Derive the dev variant so its background, bolt and materials cannot drift
# from the release icon. Keep the indicator opaque in both light and dark mode.
SWIFT_MODULECACHE_PATH="${work_dir}/swift-module-cache" \
CLANG_MODULE_CACHE_PATH="${work_dir}/clang-module-cache" \
swift - "${source_icon}" "${dev_icon}" "${assets_dir}/DevIndicator.svg" <<'SWIFT'
import Foundation

let sourceURL = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
let devURL = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
let indicatorURL = URL(fileURLWithPath: CommandLine.arguments[3])
try FileManager.default.copyItem(at: sourceURL, to: devURL)
try FileManager.default.copyItem(
    at: indicatorURL, to: devURL.appendingPathComponent("Assets/DevIndicator.svg")
)
let jsonURL = devURL.appendingPathComponent("icon.json")
guard var document = try JSONSerialization.jsonObject(with: Data(contentsOf: jsonURL)) as? [String: Any],
      var groups = document["groups"] as? [[String: Any]] else {
    fatalError("invalid Taskmaster Icon Composer document")
}
groups.insert([
    "name": "Development indicator",
    "layers": [["image-name": "DevIndicator.svg", "name": "Amber indicator", "glass": false]],
    "specular": false,
    "shadow": ["kind": "none", "opacity": 0.0],
    "translucency": ["enabled": false, "value": 0.0],
], at: 0)
document["groups"] = groups
try JSONSerialization.data(withJSONObject: document, options: [.prettyPrinted, .sortedKeys])
    .write(to: jsonURL, options: .atomic)
SWIFT

compile_icon() {
    local source="$1"
    local name="$2"
    local variant="$3"
    local output="${work_dir}/${variant}"
    mkdir -p "${output}"
    xcrun actool "${source}" \
        --compile "${output}" \
        --output-format human-readable-text --warnings --notices \
        --output-partial-info-plist "${work_dir}/${variant}.plist" \
        --app-icon "${name}" --standalone-icon-behavior all \
        --minimum-deployment-target 15.0 --platform macosx --target-device mac
    if [[ ! -s "${output}/Assets.car" || ! -s "${output}/${name}.icns" \
        || "$(plutil -extract CFBundleIconName raw "${work_dir}/${variant}.plist")" != "${name}" ]]; then
        echo "error: Apple icon compiler did not produce ${name} resources" >&2
        exit 1
    fi
    iconutil -c iconset -o "${work_dir}/${variant}.iconset" "${output}/${name}.icns"
}

compile_icon "${source_icon}" TaskmasterIcon release
compile_icon "${dev_icon}" TaskmasterDevIcon dev

# Publish only after both variants have compiled and their ICNS families have
# been decoded successfully. Packaging always ships the matching resource pair.
mkdir -p "${assets_dir}/icon-assets/release" "${assets_dir}/icon-assets/dev"
cp "${work_dir}/release/TaskmasterIcon.icns" "${assets_dir}/icon.icns"
cp "${work_dir}/dev/TaskmasterDevIcon.icns" "${assets_dir}/dev-icon.icns"
cp "${work_dir}/release/Assets.car" "${assets_dir}/icon-assets/release/Assets.car"
cp "${work_dir}/dev/Assets.car" "${assets_dir}/icon-assets/dev/Assets.car"
echo "Generated release and development layered catalogs and ICNS fallbacks"
