#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORKSPACE="${REPO_ROOT}/zeus"
DEV_DIR="${HOME}/.zeus-companion-dev"
CERTS_DIR="${DEV_DIR}/certs"
CERT_FILE="${CERTS_DIR}/localhost.pem"
KEY_FILE="${CERTS_DIR}/localhost-key.pem"
PAIR_FILE="${DEV_DIR}/pair.json"
PAIR_HTTPS_FILE="${DEV_DIR}/pair-https.json"
LISTEN_PORT="${COMPANION_PORT:-8443}"

# Ensure required commands exist
for tool in cargo node; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "error: ${tool} is required to run the companion development server." >&2
    exit 1
  fi
done

# Create owner-only directory
mkdir -m 700 -p "${CERTS_DIR}"

# Reuse existing certificates if available from previous setup or generate fresh with mkcert
if [[ ! -f "${CERT_FILE}" || ! -f "${KEY_FILE}" ]]; then
  if [[ -f "${HOME}/.zeus-test/certs/localhost+1.pem" && -f "${HOME}/.zeus-test/certs/localhost+1-key.pem" ]]; then
    cp "${HOME}/.zeus-test/certs/localhost+1.pem" "${CERT_FILE}"
    cp "${HOME}/.zeus-test/certs/localhost+1-key.pem" "${KEY_FILE}"
    chmod 600 "${CERT_FILE}" "${KEY_FILE}"
  elif command -v mkcert >/dev/null 2>&1; then
    echo "==> Generating local development certificate with mkcert..."
    (
      cd "${CERTS_DIR}"
      mkcert -cert-file "${CERT_FILE}" -key-file "${KEY_FILE}" localhost 127.0.0.1 ::1
      chmod 600 "${CERT_FILE}" "${KEY_FILE}"
    )
    if command -v xcrun >/dev/null 2>&1 && xcrun simctl list devices | grep -q "Booted"; then
      echo "==> Installing mkcert root CA into booted iOS Simulator..."
      xcrun simctl keychain booted add-root-cert "$(mkcert -CAROOT)/rootCA.pem" 2>/dev/null || true
    fi
  else
    echo "error: No TLS certificate found." >&2
    echo "Please install mkcert to generate trusted local certificates:" >&2
    echo "  brew install mkcert" >&2
    echo "  mkcert -install" >&2
    exit 1
  fi
fi

# Clean up stale pairing files
rm -f "${PAIR_FILE}" "${PAIR_HTTPS_FILE}"

echo "==> Starting HTTPS reverse proxy on port ${LISTEN_PORT}..."
CERT_PATH="${CERT_FILE}" KEY_PATH="${KEY_FILE}" PAIR_PATH="${PAIR_FILE}" PAIR_HTTPS_PATH="${PAIR_HTTPS_FILE}" LISTEN_PORT="${LISTEN_PORT}" node "${SCRIPT_DIR}/companion-proxy.mjs" &
PROXY_PID=$!

cleanup() {
  echo ""
  echo "==> Stopping companion development server..."
  kill "${PROXY_PID}" 2>/dev/null || true
  wait "${PROXY_PID}" 2>/dev/null || true
  rm -f "${PAIR_FILE}" "${PAIR_HTTPS_FILE}"
}
trap cleanup EXIT INT TERM

echo "==> Starting Zeus Companion Engine fixture..."
cd "${WORKSPACE}"
cargo run -p zeus-companion --example fixture -- "${PAIR_FILE}"
