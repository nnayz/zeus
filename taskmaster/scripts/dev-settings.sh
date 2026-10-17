#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tab="${1:-general}"

case "${tab}" in
    -h|--help)
        echo "Usage: scripts/dev-settings.sh [TAB] [DEV_ARGS...]"
        echo "Watch Settings on general (default), terminal, resources, or remote."
        echo "Remaining arguments go to dev.sh. Ctrl+C stops the watcher."
        exit 0
        ;;
    general|terminal|resources|remote) ;;
    *)
        echo "error: unknown Settings tab: ${tab}" >&2
        exit 2
        ;;
esac

if [[ $# -gt 0 ]]; then
    shift
fi

exec "${script_dir}/dev-ui.sh" --settings "${tab}" "$@"
