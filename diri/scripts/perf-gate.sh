#!/usr/bin/env bash
#
# Packaged-release memory/idle-CPU gate for diri.app.
#
# The probe launches the bundle executable directly so `$!` is the exact
# process it owns. It never searches for or signals somebody else's Diri.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace_dir="$(cd "${script_dir}/.." && pwd)"
app_path="${DIRI_PERF_APP:-${workspace_dir}/dist/diri.app}"
scenario="all"
live_daemon=0

usage() {
    cat <<'EOF'
usage: diri/scripts/perf-gate.sh [--app PATH] [--scenario normal|large|all] [--live-daemon]

By default the deterministic stress sidebar fixture is used, so the probe
cannot resize or otherwise interact with a user's selected live PTY.
--live-daemon opts into measuring the user's real session set.

Environment budgets/tuning:
  DIRI_PERF_NORMAL_MAX_MB       normal-window footprint ceiling (default 80)
  DIRI_PERF_LARGE_MAX_MB        1800x1100 footprint ceiling (default 140)
  DIRI_PERF_IDLE_AVG_CPU        mean idle CPU percent ceiling (default 0.75)
  DIRI_PERF_IDLE_PEAK_CPU       peak idle CPU percent ceiling (default 1.0)
  DIRI_PERF_SETTLE_SECONDS      wait before measuring (default 30)
  DIRI_PERF_CPU_SAMPLES         one-second top samples, including warmup (default 7)
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --app)
            [[ $# -ge 2 ]] || { usage >&2; exit 2; }
            app_path="$2"
            shift 2
            ;;
        --scenario)
            [[ $# -ge 2 ]] || { usage >&2; exit 2; }
            scenario="$2"
            shift 2
            ;;
        --live-daemon)
            live_daemon=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

case "$scenario" in
    normal|large|all) ;;
    *)
        echo "error: scenario must be normal, large, or all" >&2
        exit 2
        ;;
esac

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "error: the packaged performance gate requires macOS" >&2
    exit 2
fi

executable="${app_path}/Contents/MacOS/diri"
if [[ ! -x "$executable" ]]; then
    echo "error: packaged executable not found at $executable" >&2
    exit 2
fi
if [[ ! -x /usr/bin/vmmap || ! -x /usr/bin/top ]]; then
    echo "error: vmmap and top are required" >&2
    exit 2
fi

normal_max_mb="${DIRI_PERF_NORMAL_MAX_MB:-80}"
large_max_mb="${DIRI_PERF_LARGE_MAX_MB:-140}"
idle_avg_cpu="${DIRI_PERF_IDLE_AVG_CPU:-0.75}"
idle_peak_cpu="${DIRI_PERF_IDLE_PEAK_CPU:-1.0}"
# GPUI/Metal can briefly own hundreds of MB while its initial render resources
# are in flight. On Apple Silicon those retire within a few seconds, but a
# generous steady-state window keeps the release result insensitive to host
# load and avoids mistaking launch churn for retained memory.
settle_seconds="${DIRI_PERF_SETTLE_SECONDS:-30}"
cpu_samples="${DIRI_PERF_CPU_SAMPLES:-7}"

probe_tmp="$(mktemp -d "${TMPDIR:-/tmp}/diri-perf.XXXXXX")"
owned_pid=""
owned_start=""

same_owned_process() {
    [[ -n "$owned_pid" && -n "$owned_start" ]] || return 1
    local current_start
    current_start="$(ps -p "$owned_pid" -o lstart= 2>/dev/null || true)"
    [[ "$current_start" == "$owned_start" ]]
}

stop_owned_process() {
    same_owned_process || return 0
    kill -TERM "$owned_pid" 2>/dev/null || true
    local attempt
    for attempt in {1..50}; do
        same_owned_process || break
        sleep 0.1
    done
    if same_owned_process; then
        kill -KILL "$owned_pid" 2>/dev/null || true
    fi
    wait "$owned_pid" 2>/dev/null || true
    owned_pid=""
    owned_start=""
}

cleanup() {
    stop_owned_process
    rm -rf "$probe_tmp"
}
trap cleanup EXIT INT TERM

physical_footprint_mb() {
    local pid="$1"
    local value
    value="$(
        /usr/bin/vmmap -summary "$pid" 2>/dev/null \
            | awk '/^Physical footprint:/ { print $3; exit }'
    )"
    [[ -n "$value" ]] || return 1
    awk -v value="$value" '
        BEGIN {
            unit = substr(value, length(value), 1)
            number = value + 0
            if (unit == "K") number /= 1024
            else if (unit == "G") number *= 1024
            else if (unit == "T") number *= 1024 * 1024
            printf "%.3f\n", number
        }
    '
}

measure_cpu() {
    local pid="$1"
    local output="$probe_tmp/top-${pid}.txt"
    /usr/bin/top -l "$cpu_samples" -s 1 -pid "$pid" -stats pid,cpu >"$output"
    awk -v pid="$pid" '
        $1 == pid {
            value = $2
            gsub(/%/, "", value)
            samples[++count] = value + 0
        }
        END {
            # The first top row is lifetime-weighted warmup; subsequent rows are
            # interval samples and represent the settled idle process.
            start = count > 1 ? 2 : 1
            for (i = start; i <= count; i++) {
                sum += samples[i]
                if (samples[i] > peak) peak = samples[i]
                used++
            }
            if (used == 0) exit 1
            printf "%.3f %.3f %d\n", sum / used, peak, used
        }
    ' "$output"
}

assert_at_most() {
    local label="$1"
    local actual="$2"
    local maximum="$3"
    if ! awk -v actual="$actual" -v maximum="$maximum" 'BEGIN { exit !(actual <= maximum) }'; then
        echo "FAIL: $label is $actual; budget is <= $maximum" >&2
        return 1
    fi
}

run_scenario() {
    local name="$1"
    local large_switch="$2"
    local memory_budget="$3"
    local log="$probe_tmp/${name}.log"
    local launch_environment=()
    if [[ "$live_daemon" != "1" ]]; then
        launch_environment+=(
            "DIRIJOR_SIDEBAR_PREVIEW=1"
            "DIRIJOR_SIDEBAR_SCENARIO=stress"
        )
    fi
    # main.rs intentionally treats the mere presence of this variable as the
    # stress-size switch, so normal must omit it rather than exporting `0`.
    if [[ "$large_switch" == "1" ]]; then
        launch_environment+=("DIRI_PERF_LARGE_WINDOW=1")
    fi

    echo "==> Probing packaged diri: $name"
    /usr/bin/env "${launch_environment[@]}" "$executable" >"$log" 2>&1 &
    owned_pid=$!
    owned_start="$(ps -p "$owned_pid" -o lstart= 2>/dev/null || true)"
    if [[ -z "$owned_start" ]]; then
        echo "error: packaged diri exited during launch; log follows" >&2
        sed -n '1,160p' "$log" >&2
        return 1
    fi

    local command
    command="$(ps -p "$owned_pid" -o comm= 2>/dev/null || true)"
    if [[ "$(basename "$command")" != "diri" ]]; then
        echo "error: PID $owned_pid is not the launched diri executable ($command)" >&2
        return 1
    fi
    echo "    PID $owned_pid; settling ${settle_seconds}s"
    sleep "$settle_seconds"
    if ! same_owned_process; then
        echo "error: packaged diri PID $owned_pid exited before measurement" >&2
        sed -n '1,160p' "$log" >&2
        return 1
    fi

    local footprint
    footprint="$(physical_footprint_mb "$owned_pid")" || {
        echo "error: vmmap did not report physical footprint for PID $owned_pid" >&2
        return 1
    }
    local cpu average peak used
    cpu="$(measure_cpu "$owned_pid")" || {
        echo "error: top did not report CPU samples for PID $owned_pid" >&2
        return 1
    }
    read -r average peak used <<<"$cpu"

    printf '    footprint %.1f MB (budget %.1f); idle CPU avg %.3f%%, peak %.3f%% (%d samples)\n' \
        "$footprint" "$memory_budget" "$average" "$peak" "$used"
    assert_at_most "$name physical footprint (MB)" "$footprint" "$memory_budget"
    assert_at_most "$name mean idle CPU (%)" "$average" "$idle_avg_cpu"
    assert_at_most "$name peak idle CPU (%)" "$peak" "$idle_peak_cpu"
    stop_owned_process
}

case "$scenario" in
    normal)
        run_scenario normal 0 "$normal_max_mb"
        ;;
    large)
        run_scenario large 1 "$large_max_mb"
        ;;
    all)
        run_scenario normal 0 "$normal_max_mb"
        run_scenario large 1 "$large_max_mb"
        ;;
esac

# Interactive latency rides the same gate: two regressions shipped in one week
# (paced grid flush, poll-paced held pump) that idle memory/CPU budgets could
# not see. Boots the packaged daemon against a private App Support; never
# touches the real fleet. SKIP_LATENCY_PROBE=1 is the escape hatch for hosts
# that cannot spawn a shell.
if [[ "${SKIP_LATENCY_PROBE:-0}" != "1" ]]; then
    "$(dirname "$0")/latency-probe.sh" --app "$app_path"
fi

echo "PASS: packaged diri is within memory and idle-CPU budgets"
