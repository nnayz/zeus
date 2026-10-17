#!/usr/bin/env bash
#
# Headless Engine + CLI acceptance smoke (issue #44, "Cross-platform Engine").
#
# Boots the Rust Engine (zeusd-rs) with no desktop app and drives it through
# the shipped zeus CLI: spawn a shell session, deliver input, read the
# terminal back, then kill the Engine and prove a restart adopts the same
# session with its terminal state restored.
#
# The whole run uses a private state root (ZEUS_APP_SUPPORT) and a restricted
# PATH whose only tmux is a failing stub, so a dependency on the user's real
# fleet or on an external multiplexer fails the smoke instead of hiding.
#
# Runs on Linux and macOS. Usage:
#
#   zeus/scripts/headless-cockpit-smoke.sh
#
# Env:
#   ZEUS_HEADLESS_TIMEOUT_SECONDS  bound on each wait (default 60)
#   ZEUS_HEADLESS_KEEP=1           keep the scratch directory for debugging

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace_dir="$(cd "${script_dir}/.." && pwd)"
cd "${workspace_dir}"

timeout_seconds="${ZEUS_HEADLESS_TIMEOUT_SECONDS:-60}"
target_dir="${CARGO_TARGET_DIR:-${workspace_dir}/target}"
engine="${target_dir}/debug/zeusd-rs"
cli="${target_dir}/debug/zeus-cli"

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
step() { printf '\n==> %s\n' "$*"; }

step "Building the headless Engine and CLI"
cargo build --locked -p zeus-engine -p zeus-cli
[[ -x "${engine}" ]] || fail "engine binary missing: ${engine}"
[[ -x "${cli}" ]] || fail "CLI binary missing: ${cli}"

scratch="$(mktemp -d "${TMPDIR:-/tmp}/zeus-headless.XXXXXX")"
support="${scratch}/state"
stub_bin="${scratch}/bin"
mkdir -p "${support}" "${stub_bin}"

# Poison stub: the Engine and CLI must never reach for tmux. A PATH lookup
# that finds this fails the run loudly; /usr/local/bin and Homebrew are not on
# the restricted PATH below at all.
printf '%s\n' '#!/bin/sh' 'echo "FAIL: tmux was invoked" >&2' 'exit 1' >"${stub_bin}/tmux"
chmod 755 "${stub_bin}/tmux"
restricted_path="${stub_bin}:/usr/bin:/bin"

engine_env=(
    env
    -u ZEUS_SESSION_ID -u ZEUS_SOCKET -u ZEUS_CLI
    -u ZEUS_MANIFESTS_DIR -u ZEUS_REMOTE_HELPER_PATH
    PATH="${restricted_path}"
    ZEUS_APP_SUPPORT="${support}"
)

cli_run() {
    "${engine_env[@]}" "${cli}" "$@"
}

daemon_pid=""
session_id=""
cleanup() {
    if [[ -n "${session_id}" ]]; then
        cli_run session release "${session_id}" --remove >/dev/null 2>&1 || true
    fi
    if [[ -n "${daemon_pid}" ]]; then
        kill "${daemon_pid}" 2>/dev/null || true
        wait "${daemon_pid}" 2>/dev/null || true
    fi
    if [[ "${ZEUS_HEADLESS_KEEP:-0}" == "1" ]]; then
        printf 'kept scratch directory: %s\n' "${scratch}" >&2
    else
        rm -rf "${scratch}"
    fi
}
trap cleanup EXIT

start_engine() {
    local log="$1"
    "${engine_env[@]}" "${engine}" >"${log}" 2>&1 &
    daemon_pid=$!
}

wait_for_socket() {
    local log="$1"
    local waited=0
    while [[ ! -S "${support}/daemon.sock" ]]; do
        kill -0 "${daemon_pid}" 2>/dev/null || {
            tail -20 "${log}" >&2 || true
            fail "Engine exited before it bound its socket"
        }
        sleep 0.1
        waited=$((waited + 1))
        [[ "${waited}" -lt "$((timeout_seconds * 10))" ]] || fail "Engine socket never appeared"
    done
}

step "Starting the headless Engine with a private state root"
start_engine "${support}/daemon.log"
wait_for_socket "${support}/daemon.log"

step "Spawning a shell session through the CLI"
# Non-JSON spawn prints "<id>  <label>  <title>"; the id is the first field.
spawn_line="$(cli_run session spawn shell --cwd "${scratch}" --title headless-smoke)"
read -r session_id _rest <<<"${spawn_line}"
[[ -n "${session_id}" ]] || fail "spawn printed no session id: ${spawn_line}"
printf 'session: %s\n' "${session_id}"

# Wait for the login shell to paint a prompt, then deliver real input.
waited=0
while [[ -z "$(cli_run session read "${session_id}" --source screen --lines 5 2>/dev/null | tr -d '[:space:]')" ]]; do
    sleep 0.1
    waited=$((waited + 1))
    [[ "${waited}" -lt "$((timeout_seconds * 10))" ]] || fail "shell never painted a prompt"
done

marker="zeus-headless-$$-$(date +%s)"
step "Delivering input and reading the terminal back"
cli_run session send "${session_id}" "echo ${marker}" >/dev/null

occurrences=0
waited=0
while [[ "${occurrences}" -lt 2 ]]; do
    # The marker must appear twice: the echoed command line and its output.
    occurrences="$(
        cli_run session read "${session_id}" --source screen --lines 40 2>/dev/null |
            grep -Fc "${marker}" || true
    )"
    sleep 0.1
    waited=$((waited + 1))
    [[ "${waited}" -lt "$((timeout_seconds * 10))" ]] || fail "input never reached the session screen"
done

step "Killing the Engine and adopting the live session"
before="$(cli_run session list --all --json)"
kill -9 "${daemon_pid}"
wait "${daemon_pid}" 2>/dev/null || true
daemon_pid=""

start_engine "${support}/daemon-adopt.log"
waited=0
while ! cli_run session list --all --json >/dev/null 2>&1; do
    kill -0 "${daemon_pid}" 2>/dev/null || fail "restarted Engine exited before serving"
    sleep 0.1
    waited=$((waited + 1))
    [[ "${waited}" -lt "$((timeout_seconds * 10))" ]] || fail "restarted Engine never served the CLI"
done

after="$(cli_run session list --all --json)"
printf '%s' "${after}" | grep -Fq "\"${session_id}\"" ||
    fail "session disappeared across the Engine restart"
printf '%s' "${after}" | grep -Fq '"status":' ||
    fail "session list lost its status field"
[[ "${before}" == *'"status":'* ]] || fail "first session list lost its status field"

screen="$(cli_run session read "${session_id}" --source screen --lines 40)"
printf '%s' "${screen}" | grep -Fq "${marker}" ||
    fail "terminal state was not restored after the Engine restart"
printf 'adopted session %s with its screen intact\n' "${session_id}"

step "Releasing the session from the CLI"
released_id="${session_id}"
cli_run session release "${released_id}" --remove --json >/dev/null
session_id=""
if cli_run session list --all --json | grep -Fq "\"${released_id}\""; then
    fail "released session is still listed"
fi

printf '\nPASS: headless Engine + CLI spawn, input, screen, adoption, and release\n'
