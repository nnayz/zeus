#!/usr/bin/env bash
# Build and launch ZeusCompanion on an iOS simulator.
#
# Usage: scripts/run.sh [--release] [--device NAME|UDID]
#
# Boots the target simulator if needed, builds the app for that simulator, then
# installs and launches it. The Simulator app is opened when possible.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ios_dir="$(cd "${script_dir}/.." && pwd)"
configuration="${ZEUS_IOS_CONFIGURATION:-Debug}"
requested_simulator="${ZEUS_IOS_SIMULATOR:-}"
bundle_id="${ZEUS_IOS_BUNDLE_ID:-com.zeus.companion}"
derived_data="${ZEUS_IOS_DERIVED_DATA:-${ios_dir}/.build}"

usage() {
    cat <<'USAGE'
Usage: scripts/run.sh [--release] [--device NAME|UDID]

Build and launch ZeusCompanion on an iOS simulator.

Options:
  --release        Build with the Release configuration.
  --device DEVICE  Simulator name (partial names are accepted) or UDID to
                   use. Defaults to the first booted iPhone, otherwise the
                   newest available iPhone.
  -h, --help       Show this help.

Environment:
  ZEUS_IOS_SIMULATOR  Simulator used when --device is absent.
  ZEUS_IOS_BUNDLE_ID  Bundle identifier to launch (default: com.zeus.companion).
  ZEUS_IOS_CONFIGURATION, ZEUS_IOS_DERIVED_DATA
                      See scripts/build.sh.
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)
            configuration="Release"
            shift
            ;;
        --device)
            if [[ $# -lt 2 ]]; then
                echo "error: --device requires a value" >&2
                usage >&2
                exit 2
            fi
            requested_simulator="$2"
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

for tool in xcrun python3; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        echo "error: ${tool} is required; install Xcode and select it with xcode-select -s /Applications/Xcode.app" >&2
        exit 1
    fi
done

# Resolve the simulator to use. An explicit request wins; otherwise prefer a
# booted iPhone and fall back to the newest available iPhone.
simulator="$(WANTED="${requested_simulator}" python3 - <<'PY'
import json, os, re, subprocess, sys

def fail(message):
    print(f"error: {message}", file=sys.stderr)
    sys.exit(1)

try:
    listing = json.loads(subprocess.check_output(
        ["xcrun", "simctl", "list", "devices", "available", "--json"], text=True))
except (OSError, subprocess.CalledProcessError) as error:
    fail(f"unable to list iOS simulators ({error}); open Xcode so it can finish installing")

devices = []
for runtime, entries in listing.get("devices", {}).items():
    match = re.search(r"iOS-(\d+)(?:-(\d+))?$", runtime)
    if not match:
        continue
    version = (int(match.group(1)), int(match.group(2) or 0))
    for entry in entries:
        if entry.get("isAvailable"):
            devices.append((version, entry))

devices.sort(key=lambda item: item[0], reverse=True)

wanted = os.environ.get("WANTED", "")
if wanted:
    query = wanted.lower()

    def score(item):
        if item["udid"].lower() == query or item["name"].lower() == query:
            return 2
        return 1 if query in item["name"].lower() else 0

    scored = sorted(((score(item), item) for _, item in devices), key=lambda pair: pair[0], reverse=True)
    matches = [item for rank, item in scored if rank > 0]
    if not matches:
        fail(f"no available simulator matches {wanted!r}")
    entry = next((item for item in matches if item["state"] == "Booted"), matches[0])
else:
    iphones = [entry for _, entry in devices if entry["name"].startswith("iPhone")]
    entry = next((item for item in iphones if item["state"] == "Booted"), None)
    if entry is None and iphones:
        entry = iphones[0]
    if entry is None:
        fail("no available iPhone simulators; install one in Xcode (Settings -> Platforms)")

print(f"{entry['udid']} {entry['name']}")
PY
)" || exit $?

udid="${simulator%% *}"
simulator_name="${simulator#* }"

echo "==> Using simulator ${simulator_name} (${udid})"
xcrun simctl bootstatus "${udid}" -b
# Best effort: install and launch also work without the Simulator UI.
open -a Simulator >/dev/null 2>&1 || true

ZEUS_IOS_CONFIGURATION="${configuration}" ZEUS_IOS_DERIVED_DATA="${derived_data}" \
    "${script_dir}/build.sh" --destination "platform=iOS Simulator,id=${udid}"

app="${derived_data}/Build/Products/${configuration}-iphonesimulator/ZeusCompanion.app"
if [[ ! -d "${app}" ]]; then
    echo "error: built app not found at ${app}" >&2
    exit 1
fi

echo "==> Installing ${app}"
xcrun simctl install "${udid}" "${app}"

echo "==> Launching ${bundle_id}"
xcrun simctl launch "${udid}" "${bundle_id}"
