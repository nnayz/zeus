#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace_dir="$(cd "${script_dir}/.." && pwd)"

if [[ -z "${ZEUS_REMOTE_SSH_TARGET:-}" ]]; then
    echo "ZEUS_REMOTE_SSH_TARGET must name a disposable SSH account" >&2
    exit 64
fi

cd "${workspace_dir}"
cargo test --locked --release --package zeus-remote --test real_ssh_soak \
    real_ssh_detach_soak_reconnects_the_same_process -- --ignored --exact --nocapture
