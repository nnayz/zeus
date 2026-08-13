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

for tool in bash cargo python3 swift; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        echo "error: ${tool} is required; see README.md" >&2
        exit 1
    fi
done

# Keep compiler caches inside the checkout. This works in sandboxed development
# environments and avoids depending on writable global Swift/Clang cache paths.
export CLANG_MODULE_CACHE_PATH="${CLANG_MODULE_CACHE_PATH:-${root}/.build/clang-module-cache}"
export SWIFTPM_MODULECACHE_OVERRIDE="${SWIFTPM_MODULECACHE_OVERRIDE:-${CLANG_MODULE_CACHE_PATH}}"
mkdir -p "${CLANG_MODULE_CACHE_PATH}"

echo "==> Shell and release publishing guards"
bash -n "${root}"/scripts/*.sh "${root}"/zeus/scripts/*.sh
bash "${root}/zeus/scripts/test-publish-github-release.sh"
bash "${root}/zeus/scripts/test-publish-homebrew-cask.sh"

echo "==> Swift engine"
swift test --package-path "${root}" --no-parallel

echo "==> Rust app"
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
    ZEUS_RUN_BROWSER_TESTS=1 swift test --package-path "${root}" --filter BrowserPoolTests
fi

echo "All checks passed."
