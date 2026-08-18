#!/usr/bin/env bash

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
run_browser=0

case "${1:-}" in
    "") ;;
    --browser) run_browser=1 ;;
    -h|--help)
        echo "usage: ./scripts/check.sh [--browser]"
        exit 0
        ;;
    *)
        echo "error: unknown option: $1" >&2
        echo "usage: ./scripts/check.sh [--browser]" >&2
        exit 2
        ;;
esac

for tool in bash cargo python3; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        echo "error: ${tool} is required; see README.md" >&2
        exit 1
    fi
done

echo "==> Shell and release publishing guards"
bash -n "${root}"/scripts/*.sh "${root}"/zeus/scripts/*.sh
bash "${root}/zeus/scripts/test-publish-github-release.sh"
bash "${root}/zeus/scripts/test-publish-homebrew-cask.sh"

echo "==> Rust workspace"
(
    cd "${root}/zeus"
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
)

if [[ "${run_browser}" == "1" ]]; then
    if ! command -v npm >/dev/null 2>&1; then
        echo "error: npm is required for --browser" >&2
        exit 1
    fi
    echo "==> Browser sidecar"
    (
        cd "${root}/sidecar"
        npm ci
        npm audit --omit=dev
        npx playwright install chromium webkit firefox
    )
    (
        cd "${root}/zeus"
        cargo test -p zeus-engine browser
    )
fi

echo "All checks passed."
