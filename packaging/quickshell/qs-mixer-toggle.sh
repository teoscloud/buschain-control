#!/usr/bin/env bash
# BusChain → Quickshell mixer toggle bridge.
# Install: ~/.config/quickshell/scripts/qs-mixer-toggle.sh  (chmod +x)
# Exit 0 = handled (tray will not fall through to GTK/egui).

set -euo pipefail

# Prefer qs IPC if your shell registers a `mixer` object.
if command -v qs >/dev/null 2>&1; then
  if qs ipc call mixer toggle 2>/dev/null; then
    exit 0
  fi
fi

# Fallback: touch a flag file your QS config can watch (optional).
RUNTIME="${XDG_RUNTIME_DIR:-/tmp}/buschain-control"
mkdir -p "$RUNTIME"
# Flip open/closed marker for FileView-based shells.
if [[ -f "$RUNTIME/mixer.open" ]]; then
  rm -f "$RUNTIME/mixer.open"
else
  printf '1' >"$RUNTIME/mixer.open"
fi
exit 0
