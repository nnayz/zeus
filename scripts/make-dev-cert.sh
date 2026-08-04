#!/usr/bin/env bash
# Creates a stable self-signed "Dirijor Dev" code-signing identity in the login
# keychain so the app's code signature (and TCC permission grants) survive
# rebuilds. Ad-hoc signing changes identity every build → macOS re-prompts for
# every permission. Run once.
set -euo pipefail

if security find-certificate -c "Dirijor Dev" >/dev/null 2>&1; then
    echo "'Dirijor Dev' cert already exists."
    exit 0
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/cert.conf" <<'CONF'
[req]
distinguished_name = dn
x509_extensions = v3
prompt = no
[dn]
CN = Dirijor Dev
[v3]
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
basicConstraints = critical, CA:false
CONF
openssl req -x509 -newkey rsa:2048 -keyout "$tmp/k.key" -out "$tmp/c.crt" \
    -days 3650 -nodes -config "$tmp/cert.conf" 2>/dev/null
openssl pkcs12 -export -inkey "$tmp/k.key" -in "$tmp/c.crt" -out "$tmp/id.p12" \
    -passout pass:dirijor -name "Dirijor Dev" 2>/dev/null
security import "$tmp/id.p12" -k ~/Library/Keychains/login.keychain-db -P dirijor -A
echo "Created 'Dirijor Dev' code-signing identity. Rebuild the app to use it."
