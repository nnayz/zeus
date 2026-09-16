#!/usr/bin/env bash
# Read-only release-spike check for the optional issue #70 power helper.
# This script does not install/register a service or change power settings.

set -u
set -o pipefail

readonly APP_IDENTIFIER="com.zeus.zeus"
readonly HELPER_IDENTIFIER="com.zeus.zeus.power-helper"
readonly HELPER_RELATIVE_PATH="Contents/Library/HelperTools/com.zeus.zeus.power-helper"
readonly EXIT_INVALID=1
readonly EXIT_USAGE=2
readonly EXIT_UNAVAILABLE=3
# Never permit environment variables to select mocked validation.
SELF_TESTING=0

fail() {
    printf 'FAIL: %s\n' "$*" >&2
}

unavailable() {
    printf 'UNAVAILABLE [power_helper_unavailable]: optional issue #70 helper is absent at %s\n' "$1" >&2
    printf 'This is expected until a power helper is approved and packaged; closed-lid mode must remain disabled.\n' >&2
}

# Tool wrappers are internal so --self-test can exercise fail-closed behavior
# without creating or trusting an ad-hoc signed executable.
lipo_archs() {
    if [[ "${SELF_TESTING:-0}" == "1" ]]; then
        [[ "${MOCK_LIPO_OK}" == "1" ]] || return 1
        printf '%s\n' "${MOCK_ARCHS}"
        return 0
    fi
    /usr/bin/lipo -archs "$1"
}

codesign_strict() {
    local kind="$1"
    local path="$2"
    if [[ "${SELF_TESTING:-0}" == "1" ]]; then
        if [[ "$kind" == "app" ]]; then
            [[ "${MOCK_APP_VERIFY_OK}" == "1" ]]
        else
            [[ "${MOCK_HELPER_VERIFY_OK}" == "1" ]]
        fi
        return
    fi
    if [[ "$kind" == "app" ]]; then
        /usr/bin/codesign --verify --deep --strict --verbose=2 "$path"
    else
        /usr/bin/codesign --verify --strict --verbose=2 "$path"
    fi
}

codesign_metadata() {
    local kind="$1"
    local path="$2"
    if [[ "${SELF_TESTING:-0}" == "1" ]]; then
        if [[ "$kind" == "app" ]]; then
            printf '%s\n' "${MOCK_APP_METADATA}"
        else
            printf '%s\n' "${MOCK_HELPER_METADATA}"
        fi
        return 0
    fi
    /usr/bin/codesign --display --verbose=4 "$path" 2>&1
}

codesign_designated_requirement() {
    local kind="$1"
    local path="$2"
    if [[ "${SELF_TESTING:-0}" == "1" ]]; then
        if [[ "$kind" == "app" ]]; then
            printf '%s\n' "${MOCK_APP_DR}"
        else
            printf '%s\n' "${MOCK_HELPER_DR}"
        fi
        return 0
    fi
    /usr/bin/codesign --display --requirements - "$path" 2>&1
}

codesign_test_requirement() {
    local kind="$1"
    local path="$2"
    local requirement="$3"
    if [[ "${SELF_TESTING:-0}" == "1" ]]; then
        if [[ "$kind" == "app" ]]; then
            [[ "${MOCK_APP_REQUIREMENT_OK}" == "1" ]]
        else
            [[ "${MOCK_HELPER_REQUIREMENT_OK}" == "1" ]]
        fi
        return
    fi
    /usr/bin/codesign --verify --strict -R="$requirement" "$path"
}

single_metadata_field() {
    local metadata="$1"
    local field="$2"
    local values count
    values="$(printf '%s\n' "$metadata" | /usr/bin/awk -F= -v key="$field" '$1 == key { print substr($0, length(key) + 2) }')"
    count="$(printf '%s\n' "$values" | /usr/bin/awk 'NF { count += 1 } END { print count + 0 }')"
    [[ "$count" == "1" ]] || return 1
    printf '%s\n' "$values"
}

has_hardened_runtime() {
    printf '%s\n' "$1" | /usr/bin/grep -Eq 'flags=[^[:space:]]*\([^)]*runtime([,)]|$)'
}

has_designated_requirement() {
    local output="$1"
    local identifier="$2"
    [[ "$output" == *"designated =>"* ]] || return 1
    [[ "$output" == *"identifier \"${identifier}\""* ]] || return 1
}

check_architectures() {
    local archs="$1"
    local arch count=0 arm64=0 x86_64=0
    for arch in $archs; do
        count=$((count + 1))
        case "$arch" in
            arm64) arm64=$((arm64 + 1)) ;;
            x86_64) x86_64=$((x86_64 + 1)) ;;
            *) return 1 ;;
        esac
    done
    [[ "$count" == "2" && "$arm64" == "1" && "$x86_64" == "1" ]]
}

check_bundle() {
    local app="$1"
    local helper="${app}/${HELPER_RELATIVE_PATH}"
    local archs app_metadata helper_metadata app_team helper_team app_id helper_id
    local app_dr helper_dr app_requirement helper_requirement

    if [[ ! -d "$app" || -L "$app" ]]; then
        fail "app bundle must be a real, non-symlink directory: $app"
        return "$EXIT_INVALID"
    fi

    # Do not search PATHs, alternate bundle directories, or installed services.
    # The fixed nested path is the only release artifact accepted by this spike.
    if [[ ! -e "$helper" && ! -L "$helper" ]]; then
        unavailable "$helper"
        return "$EXIT_UNAVAILABLE"
    fi
    if [[ ! -f "$helper" || -L "$helper" ]]; then
        fail "helper must be a regular, non-symlink file at the exact package path: $helper"
        return "$EXIT_INVALID"
    fi

    if [[ "${SELF_TESTING:-0}" != "1" ]]; then
        if [[ "$(/usr/bin/uname -s)" != "Darwin" ]]; then
            fail "real bundle verification requires macOS; use --self-test for mocked control-flow tests"
            return "$EXIT_USAGE"
        fi
        if [[ ! -x /usr/bin/lipo || ! -x /usr/bin/codesign ]]; then
            fail "required Apple tools are unavailable at /usr/bin/lipo and /usr/bin/codesign"
            return "$EXIT_USAGE"
        fi
    fi

    if ! archs="$(lipo_archs "$helper" 2>&1)"; then
        fail "lipo could not read helper architectures: $archs"
        return "$EXIT_INVALID"
    fi
    if ! check_architectures "$archs"; then
        fail "helper must contain exactly arm64 and x86_64 slices; found: ${archs:-<none>}"
        return "$EXIT_INVALID"
    fi

    if ! codesign_strict helper "$helper" >/dev/null 2>&1; then
        fail "helper failed codesign --verify --strict"
        return "$EXIT_INVALID"
    fi
    if ! codesign_strict app "$app" >/dev/null 2>&1; then
        fail "app failed codesign --verify --deep --strict"
        return "$EXIT_INVALID"
    fi

    if ! app_metadata="$(codesign_metadata app "$app")"; then
        fail "could not read app signing metadata"
        return "$EXIT_INVALID"
    fi
    if ! helper_metadata="$(codesign_metadata helper "$helper")"; then
        fail "could not read helper signing metadata"
        return "$EXIT_INVALID"
    fi
    if ! app_id="$(single_metadata_field "$app_metadata" Identifier)" || [[ "$app_id" != "$APP_IDENTIFIER" ]]; then
        fail "app signing identifier must be $APP_IDENTIFIER (found ${app_id:-<missing-or-ambiguous>})"
        return "$EXIT_INVALID"
    fi
    if ! helper_id="$(single_metadata_field "$helper_metadata" Identifier)" || [[ "$helper_id" != "$HELPER_IDENTIFIER" ]]; then
        fail "helper signing identifier must be $HELPER_IDENTIFIER (found ${helper_id:-<missing-or-ambiguous>})"
        return "$EXIT_INVALID"
    fi
    if ! app_team="$(single_metadata_field "$app_metadata" TeamIdentifier)" || ! [[ "$app_team" =~ ^[A-Z0-9]{10}$ ]]; then
        fail "app has no single valid 10-character Team ID"
        return "$EXIT_INVALID"
    fi
    if ! helper_team="$(single_metadata_field "$helper_metadata" TeamIdentifier)" || [[ "$helper_team" != "$app_team" ]]; then
        fail "helper Team ID does not exactly match the app Team ID"
        return "$EXIT_INVALID"
    fi
    if ! has_hardened_runtime "$app_metadata"; then
        fail "app signing flags do not show hardened runtime"
        return "$EXIT_INVALID"
    fi
    if ! has_hardened_runtime "$helper_metadata"; then
        fail "helper signing flags do not show hardened runtime"
        return "$EXIT_INVALID"
    fi

    if ! app_dr="$(codesign_designated_requirement app "$app")" || ! has_designated_requirement "$app_dr" "$APP_IDENTIFIER"; then
        fail "app has no parseable designated requirement for $APP_IDENTIFIER"
        return "$EXIT_INVALID"
    fi
    if ! helper_dr="$(codesign_designated_requirement helper "$helper")" || ! has_designated_requirement "$helper_dr" "$HELPER_IDENTIFIER"; then
        fail "helper has no parseable designated requirement for $HELPER_IDENTIFIER"
        return "$EXIT_INVALID"
    fi

    # Do not compare DR strings: the app and helper have different identifiers.
    # Instead, make codesign evaluate the same Apple anchor + app Team ID policy
    # for each exact identifier. This binds both designated identities to the
    # app's signing authority while preserving distinct executable identities.
    app_requirement="identifier \"${APP_IDENTIFIER}\" and anchor apple generic and certificate leaf[subject.OU] = \"${app_team}\""
    helper_requirement="identifier \"${HELPER_IDENTIFIER}\" and anchor apple generic and certificate leaf[subject.OU] = \"${app_team}\""
    if ! codesign_test_requirement app "$app" "$app_requirement" >/dev/null 2>&1; then
        fail "app does not satisfy its Apple-anchor/Team-ID designated requirement"
        return "$EXIT_INVALID"
    fi
    if ! codesign_test_requirement helper "$helper" "$helper_requirement" >/dev/null 2>&1; then
        fail "helper designated identity does not match the app signing authority"
        return "$EXIT_INVALID"
    fi

    printf 'PASS: optional power helper package seam is valid\n'
    printf '  app:    %s\n' "$app"
    printf '  helper: %s\n' "$helper"
    printf '  slices: arm64 x86_64\n'
    printf '  Team ID: %s\n' "$app_team"
    return 0
}

reset_mocks() {
    MOCK_LIPO_OK=1
    MOCK_ARCHS="arm64 x86_64"
    MOCK_APP_VERIFY_OK=1
    MOCK_HELPER_VERIFY_OK=1
    MOCK_APP_REQUIREMENT_OK=1
    MOCK_HELPER_REQUIREMENT_OK=1
    MOCK_APP_METADATA='Identifier=com.zeus.zeus
TeamIdentifier=ABCDE12345
CodeDirectory v=20500 flags=0x10000(runtime)'
    MOCK_HELPER_METADATA='Identifier=com.zeus.zeus.power-helper
TeamIdentifier=ABCDE12345
CodeDirectory v=20500 flags=0x10000(runtime)'
    MOCK_APP_DR='designated => identifier "com.zeus.zeus" and anchor apple generic and certificate leaf[subject.OU] = "ABCDE12345"'
    MOCK_HELPER_DR='designated => identifier "com.zeus.zeus.power-helper" and anchor apple generic and certificate leaf[subject.OU] = "ABCDE12345"'
}

run_self_test() {
    local root app helper log passed=0 total=0 rc expected name
    SELF_TESTING=1
    root="$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/zeus-power-package-test.XXXXXX")" || return 1
    app="${root}/zeus.app"
    helper="${app}/${HELPER_RELATIVE_PATH}"
    /bin/mkdir -p "$(/usr/bin/dirname "$helper")"
    log="${root}/case.log"
    SELF_TEST_ROOT="$root"
    trap 'if [[ -n "${SELF_TEST_ROOT:-}" ]]; then /bin/rm -rf "$SELF_TEST_ROOT"; fi' EXIT HUP INT TERM

    self_test_case() {
        name="$1"
        expected="$2"
        total=$((total + 1))
        check_bundle "$app" >"$log" 2>&1
        rc=$?
        if [[ "$rc" == "$expected" ]]; then
            printf 'ok %s - %s\n' "$total" "$name"
            passed=$((passed + 1))
        else
            printf 'not ok %s - %s (expected %s, got %s)\n' "$total" "$name" "$expected" "$rc" >&2
            /bin/cat "$log" >&2
        fi
    }

    reset_mocks
    self_test_case "missing helper is unavailable and nonzero" "$EXIT_UNAVAILABLE"

    : >"$helper"
    self_test_case "valid mocked release bundle passes" 0

    /bin/rm -f "$helper"
    /bin/ln -s /dev/null "$helper"
    self_test_case "symlink helper is rejected" "$EXIT_INVALID"
    /bin/rm -f "$helper"
    : >"$helper"

    reset_mocks
    MOCK_ARCHS="arm64"
    self_test_case "missing x86_64 slice is rejected" "$EXIT_INVALID"

    reset_mocks
    MOCK_ARCHS="arm64 x86_64 ppc"
    self_test_case "unexpected architecture is rejected" "$EXIT_INVALID"

    reset_mocks
    MOCK_HELPER_VERIFY_OK=0
    self_test_case "invalid helper signature is rejected" "$EXIT_INVALID"

    reset_mocks
    MOCK_HELPER_METADATA='Identifier=com.zeus.zeus.power-helper
TeamIdentifier=WRONG12345
CodeDirectory v=20500 flags=0x10000(runtime)'
    self_test_case "wrong helper Team ID is rejected" "$EXIT_INVALID"

    reset_mocks
    MOCK_HELPER_REQUIREMENT_OK=0
    self_test_case "designated requirement mismatch is rejected" "$EXIT_INVALID"

    reset_mocks
    MOCK_HELPER_METADATA='Identifier=com.zeus.zeus.power-helper
TeamIdentifier=ABCDE12345
CodeDirectory v=20500 flags=0x0(none)'
    self_test_case "missing helper hardened runtime is rejected" "$EXIT_INVALID"

    reset_mocks
    MOCK_APP_METADATA='Identifier=com.zeus.zeus
TeamIdentifier=ABCDE12345
CodeDirectory v=20500 flags=0x0(none)'
    self_test_case "missing app hardened runtime is rejected" "$EXIT_INVALID"

    if [[ "$passed" != "$total" ]]; then
        printf 'self-test: %s/%s passed\n' "$passed" "$total" >&2
        trap - EXIT HUP INT TERM
        /bin/rm -rf "$root"
        return 1
    fi
    printf 'self-test: all %s cases passed\n' "$total"
    trap - EXIT HUP INT TERM
    /bin/rm -rf "$root"
    return 0
}

usage() {
    cat >&2 <<EOF
usage: $0 /path/to/zeus.app
       $0 --self-test

Read-only checks for the future optional issue #70 helper at:
  ${HELPER_RELATIVE_PATH}

Exit 0 means all checks passed. Exit ${EXIT_UNAVAILABLE} means the helper is
absent, which is expected before implementation and must disable the feature.
EOF
}

if [[ $# == 1 && "$1" == "--self-test" ]]; then
    run_self_test
    exit $?
fi
if [[ $# != 1 ]]; then
    usage
    exit "$EXIT_USAGE"
fi
check_bundle "$1"
exit $?
