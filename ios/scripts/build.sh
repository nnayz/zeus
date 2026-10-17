#!/usr/bin/env bash
# Build the ZeusCompanion iOS app with xcodebuild.
#
# Usage: scripts/build.sh [--release] [--destination DESTINATION]

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ios_dir="$(cd "${script_dir}/.." && pwd)"
project="${ios_dir}/ZeusCompanion/ZeusCompanion.xcodeproj"
scheme="ZeusCompanion"
configuration="${ZEUS_IOS_CONFIGURATION:-Debug}"
destination="${ZEUS_IOS_DESTINATION:-generic/platform=iOS Simulator}"
derived_data="${ZEUS_IOS_DERIVED_DATA:-${ios_dir}/.build}"

usage() {
    cat <<'USAGE'
Usage: scripts/build.sh [--release] [--destination DESTINATION]

Build the ZeusCompanion iOS app with xcodebuild.

Options:
  --release                  Build with the Release configuration.
  --destination DESTINATION  xcodebuild destination
                             (default: generic/platform=iOS Simulator).
  -h, --help                 Show this help.

Environment:
  ZEUS_IOS_CONFIGURATION  Debug or Release (default: Debug).
  ZEUS_IOS_DESTINATION    Destination used when --destination is absent.
  ZEUS_IOS_DERIVED_DATA   Derived data directory (default: ios/.build).
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)
            configuration="Release"
            shift
            ;;
        --destination)
            if [[ $# -lt 2 ]]; then
                echo "error: --destination requires a value" >&2
                usage >&2
                exit 2
            fi
            destination="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "error: unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

for tool in xcodebuild xcrun; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        echo "error: ${tool} is required; install Xcode and select it with xcode-select -s /Applications/Xcode.app" >&2
        exit 1
    fi
done

if [[ ! -d "${project}" ]]; then
    echo "error: Xcode project not found at ${project}" >&2
    exit 1
fi

mkdir -p "${derived_data}"

echo "==> Building ${scheme} (${configuration}) for ${destination}"
xcodebuild \
    -project "${project}" \
    -scheme "${scheme}" \
    -configuration "${configuration}" \
    -destination "${destination}" \
    -derivedDataPath "${derived_data}" \
    build
