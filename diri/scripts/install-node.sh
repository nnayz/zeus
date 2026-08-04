#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PROJECT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
CARGO_BIN=${CARGO_BIN:-cargo}
INSTALL_BIN=${INSTALL_BIN:-"$HOME/.local/bin"}
SYSTEMD_USER_DIR=${SYSTEMD_USER_DIR:-"$HOME/.config/systemd/user"}

cd "$PROJECT_DIR"
"$CARGO_BIN" build --release -p diri-node
mkdir -p \
    "$INSTALL_BIN" \
    "$SYSTEMD_USER_DIR" \
    "$HOME/.config/dirijor" \
    "$HOME/.local/share/dirijor/node"
install -m 0755 "target/release/diri-node" "$INSTALL_BIN/diri-node"
install -m 0644 "infra/diri-node.service" "$SYSTEMD_USER_DIR/diri-node.service"
chmod 700 "$HOME/.config/dirijor"
chmod 700 "$HOME/.local/share/dirijor/node"

if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload
fi

printf '%s\n' "Installed $INSTALL_BIN/diri-node"
printf '%s\n' "Next: set DIRIJOR_NODE_LISTEN in ~/.config/dirijor/node.env, then run:"
printf '%s\n' "  systemctl --user enable --now diri-node"
printf '%s\n' "  diri-node init"
