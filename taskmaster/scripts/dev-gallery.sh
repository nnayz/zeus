#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "${1:-}" in
    -h|--help)
        echo "Usage: scripts/dev-gallery.sh [CARGO_RUN_ARGS...]"
        echo "Watch and reopen the shared UI component gallery."
        echo "Remaining arguments go to cargo run, e.g. --release."
        echo "Ctrl+C stops the watcher."
        exit 0
        ;;
esac

exec "${script_dir}/dev-watch.sh" \
    cargo run --package taskmaster-ui --example gallery "$@"
