#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace_dir="$(cd "${script_dir}/.." && pwd)"
assets_dir="${workspace_dir}/assets"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/diri-icon.XXXXXX")"
base_png="${work_dir}/icon-1024.png"
iconset_dir="${work_dir}/diri.iconset"

cleanup() {
    if [[ "${DIRI_KEEP_ICON_WORK_DIR:-0}" == "1" ]]; then
        echo "Kept icon workspace at ${work_dir}" >&2
        return
    fi
    rm -rf "${work_dir}"
}
trap cleanup EXIT

mkdir -p "${assets_dir}" "${iconset_dir}"

SWIFT_MODULECACHE_PATH="${work_dir}/swift-module-cache" \
CLANG_MODULE_CACHE_PATH="${work_dir}/clang-module-cache" \
swift - "${base_png}" <<'SWIFT'
import AppKit

let canvas = NSSize(width: 1024, height: 1024)
let outputURL = URL(fileURLWithPath: CommandLine.arguments[1])

func color(_ hex: UInt32, alpha: CGFloat = 1) -> NSColor {
    NSColor(
        srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
        green: CGFloat((hex >> 8) & 0xff) / 255,
        blue: CGFloat(hex & 0xff) / 255,
        alpha: alpha
    )
}

let image = NSImage(size: canvas, flipped: false) { _ in
    NSGraphicsContext.current?.imageInterpolation = .high

    let shadow = NSBezierPath(
        roundedRect: NSRect(x: 74, y: 54, width: 876, height: 876),
        xRadius: 194,
        yRadius: 194
    )
    color(0x05070b, alpha: 0.40).setFill()
    shadow.fill()

    let tile = NSBezierPath(
        roundedRect: NSRect(x: 64, y: 72, width: 896, height: 896),
        xRadius: 198,
        yRadius: 198
    )
    tile.addClip()
    NSGradient(
        starting: color(0x252a37),
        ending: color(0x10131b)
    )!.draw(in: tile, angle: -90)

    let topSheen = NSBezierPath(
        roundedRect: NSRect(x: 86, y: 540, width: 852, height: 398),
        xRadius: 170,
        yRadius: 170
    )
    color(0xffffff, alpha: 0.025).setFill()
    topSheen.fill()

    let innerBorder = NSBezierPath(
        roundedRect: NSRect(x: 76, y: 84, width: 872, height: 872),
        xRadius: 188,
        yRadius: 188
    )
    innerBorder.lineWidth = 6
    color(0xffffff, alpha: 0.07).setStroke()
    innerBorder.stroke()

    let motif = color(0xd97757)

    let chevron = NSBezierPath()
    chevron.move(to: NSPoint(x: 326, y: 650))
    chevron.line(to: NSPoint(x: 508, y: 512))
    chevron.line(to: NSPoint(x: 326, y: 374))
    chevron.lineWidth = 70
    chevron.lineCapStyle = .round
    chevron.lineJoinStyle = .round
    motif.setStroke()
    chevron.stroke()

    let cursor = NSBezierPath()
    cursor.move(to: NSPoint(x: 558, y: 378))
    cursor.line(to: NSPoint(x: 738, y: 378))
    cursor.lineWidth = 62
    cursor.lineCapStyle = .round
    color(0xd97757, alpha: 0.92).setStroke()
    cursor.stroke()

    return true
}

guard
    let tiff = image.tiffRepresentation,
    let bitmap = NSBitmapImageRep(data: tiff),
    let png = bitmap.representation(using: .png, properties: [:])
else {
    fatalError("could not render diri icon")
}

try png.write(to: outputURL, options: .atomic)
SWIFT

while read -r pixels filename; do
    sips -z "${pixels}" "${pixels}" "${base_png}" --out "${iconset_dir}/${filename}" >/dev/null
done <<'SIZES'
16 icon_16x16.png
32 icon_16x16@2x.png
32 icon_32x32.png
64 icon_32x32@2x.png
128 icon_128x128.png
256 icon_128x128@2x.png
256 icon_256x256.png
512 icon_256x256@2x.png
512 icon_512x512.png
1024 icon_512x512@2x.png
SIZES

SWIFT_MODULECACHE_PATH="${work_dir}/swift-module-cache" \
CLANG_MODULE_CACHE_PATH="${work_dir}/clang-module-cache" \
swift - "${iconset_dir}" "${assets_dir}/icon.icns" <<'SWIFT'
import Foundation

let iconset = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
let output = URL(fileURLWithPath: CommandLine.arguments[2])
let entries = [
    ("icp4", "icon_16x16.png"),
    ("ic11", "icon_16x16@2x.png"),
    ("icp5", "icon_32x32.png"),
    ("ic12", "icon_32x32@2x.png"),
    ("ic07", "icon_128x128.png"),
    ("ic13", "icon_128x128@2x.png"),
    ("ic08", "icon_256x256.png"),
    ("ic14", "icon_256x256@2x.png"),
    ("ic09", "icon_512x512.png"),
    ("ic10", "icon_512x512@2x.png"),
]

func bigEndianBytes(_ value: Int) -> [UInt8] {
    let encoded = UInt32(value).bigEndian
    return withUnsafeBytes(of: encoded) { Array($0) }
}

var chunks = Data()
for (type, filename) in entries {
    let png = try Data(contentsOf: iconset.appendingPathComponent(filename))
    chunks.append(contentsOf: type.utf8)
    chunks.append(contentsOf: bigEndianBytes(png.count + 8))
    chunks.append(png)
}

var family = Data("icns".utf8)
family.append(contentsOf: bigEndianBytes(chunks.count + 8))
family.append(chunks)
try family.write(to: output, options: .atomic)
SWIFT

# iconutil on macOS 26 cannot reliably compile newly rendered iconsets, but it
# can parse the deterministic ICNS family above. Extracting it is our format
# validation step and keeps the generator tied to Apple's icon tooling.
iconutil -c iconset -o "${work_dir}/validated.iconset" "${assets_dir}/icon.icns"
echo "Generated ${assets_dir}/icon.icns"
