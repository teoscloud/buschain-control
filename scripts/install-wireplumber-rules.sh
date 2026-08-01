#!/usr/bin/env bash
# Install BusChain WirePlumber seal rules for the current user.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/pipewire/wireplumber/wireplumber.conf.d/51-buschain-seal-helpers.conf"
DEST_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/wireplumber/wireplumber.conf.d"
mkdir -p "$DEST_DIR"
cp -f "$SRC" "$DEST_DIR/51-buschain-seal-helpers.conf"
echo "Installed $DEST_DIR/51-buschain-seal-helpers.conf"
echo "Restarting wireplumber…"
systemctl --user restart wireplumber
echo "Done. Public sinks stay Master + virtual outputs; helpers stay sealed."
