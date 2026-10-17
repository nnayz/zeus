#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
scenario="typical"

usage() {
    cat <<'USAGE'
Usage: scripts/dev-ui.sh [--scenario NAME] [DEV_ARGS...]

Watch, rebuild, and reopen the workbench with deterministic preview data.

Options:
  --scenario NAME  typical (default), stress, empty, or artifacts.
  -h, --help       Show this help.

Remaining arguments go to dev.sh, e.g. --settings terminal or --release.
Save to rebuild; Ctrl+C stops the watcher. Rust changes require compilation.
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --scenario)
            if [[ $# -lt 2 ]]; then
                echo "error: --scenario requires a name" >&2
                exit 2
            fi
            scenario="$2"
            shift 2
            ;;
        --scenario=*)
            scenario="${1#*=}"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *) break ;;
    esac
done

case "${scenario}" in
    typical|stress|empty|artifacts) ;;
    *)
        echo "error: unknown preview scenario: ${scenario}" >&2
        usage >&2
        exit 2
        ;;
esac

export TASKMASTER_SIDEBAR_PREVIEW=1
export TASKMASTER_SIDEBAR_SCENARIO="${scenario}"
# Open the workbench by default even if a previous preview exported a tab.
unset TASKMASTER_SETTINGS_PREVIEW
exec "${script_dir}/dev-watch.sh" "${script_dir}/dev.sh" "$@"
