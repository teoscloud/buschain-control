# GTK mixer (layer-shell panel)

QS-like tray / Waybar popup (`buschain-mixer-gtk`) for general desktops.
Preferred after Quickshell when the binary is available.

| Path | Role |
|------|------|
| [`legacy/`](legacy/) | Python + CSS sources (Playback / Tracks / Output / Input) |
| [`buschain-mixer-gtk`](buschain-mixer-gtk) | Local/dev launcher |
| [`../nix/gtk-mixer.nix`](../nix/gtk-mixer.nix) | Packaged wrapped binary |

Opt out: `BUSCHAIN_CONTROL_USE_GTK_MIXER=0`.

Popup order (tray / `buschain-ctl popup` / `buschain-waybar popup` → ctl):

1. Quickshell — if `BUSCHAIN_CONTROL_QS_MIXER=1`, toggle script, or `qs` on PATH
2. GTK layer-shell — when `buschain-mixer-gtk` is available
3. egui — `buschain-control --popup` (last resort)

Contract: [`docs/HANDOVER-QUICKSHELL.md`](../../docs/HANDOVER-QUICKSHELL.md).
