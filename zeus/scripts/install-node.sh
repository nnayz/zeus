#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PROJECT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
CARGO_BIN=${CARGO_BIN:-cargo}
INSTALL_BIN=${INSTALL_BIN:-"$HOME/.local/bin"}
SYSTEMD_USER_DIR=${SYSTEMD_USER_DIR:-"$HOME/.config/systemd/user"}

cd "$PROJECT_DIR"
"$CARGO_BIN" build --release -p zeus-node
mkdir -p \
    "$INSTALL_BIN" \
    "$SYSTEMD_USER_DIR" \
    "$HOME/.config/zeus" \
    "$HOME/.local/share/zeus/node"
install -m 0755 "target/release/zeus-node" "$INSTALL_BIN/zeus-node"
install -m 0644 "infra/zeus-node.service" "$SYSTEMD_USER_DIR/zeus-node.service"
chmod 700 "$HOME/.config/zeus"
chmod 700 "$HOME/.local/share/zeus/node"

if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload
fi

printf '%s\n' "Installed $INSTALL_BIN/zeus-node"
printf '%s\n' "Next: set ZEUS_NODE_LISTEN in ~/.config/zeus/node.env, then run:"
printf '%s\n' "  systemctl --user enable --now zeus-node"
printf '%s\n' "  zeus-node init"
