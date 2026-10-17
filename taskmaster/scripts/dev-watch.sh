#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace_dir="$(cd "${script_dir}/.." && pwd)"

case "${1:-}" in
    ""|-h|--help)
        echo "Usage: scripts/dev-watch.sh COMMAND [ARGS...]"
        echo "Watch Rust sources, assets, scripts, and workspace configuration."
        echo "Use dev-ui.sh, dev-settings.sh, or dev-gallery.sh for UI previews."
        if [[ $# -eq 0 ]]; then
            exit 2
        fi
        exit 0
        ;;
esac

if ! command -v watchexec >/dev/null 2>&1; then
    echo "error: watchexec is required; install it once with: brew install watchexec" >&2
    exit 1
fi

cd "${workspace_dir}"
echo "==> Watching UI sources; save to rebuild and restart. Ctrl+C stops the watcher."

# Watch inputs explicitly so Cargo output and the fresh dev app bundles do not
# trigger rebuild loops. Direct execution preserves argument boundaries even
# when the checkout path or forwarded arguments contain spaces.
exec watchexec \
    --restart \
    --shell=none \
    --debounce 300ms \
    --watch crates \
    --watch assets \
    --watch scripts \
    --watch Cargo.toml \
    --watch Cargo.lock \
    --watch rust-toolchain.toml \
    -- "$@"
