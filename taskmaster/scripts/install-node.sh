#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PROJECT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
CARGO_BIN=${CARGO_BIN:-cargo}
INSTALL_BIN=${INSTALL_BIN:-"$HOME/.local/bin"}
SYSTEMD_USER_DIR=${SYSTEMD_USER_DIR:-"$HOME/.config/systemd/user"}

cd "$PROJECT_DIR"
"$CARGO_BIN" build --release -p taskmaster-node
mkdir -p \
    "$INSTALL_BIN" \
    "$SYSTEMD_USER_DIR" \
    "$HOME/.config/taskmaster" \
    "$HOME/.local/share/taskmaster/node"
install -m 0755 "target/release/taskmaster-node" "$INSTALL_BIN/taskmaster-node"
install -m 0644 "infra/taskmaster-node.service" "$SYSTEMD_USER_DIR/taskmaster-node.service"
chmod 700 "$HOME/.config/taskmaster"
chmod 700 "$HOME/.local/share/taskmaster/node"

if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload
fi

printf '%s\n' "Installed $INSTALL_BIN/taskmaster-node"
printf '%s\n' "Next: set TASKMASTER_NODE_LISTEN in ~/.config/taskmaster/node.env, then run:"
printf '%s\n' "  systemctl --user enable --now taskmaster-node"
printf '%s\n' "  taskmaster-node init"
