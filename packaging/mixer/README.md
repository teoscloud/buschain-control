# GTK mixer (layer-shell panel)

Preferred waybar / tray popup when available (`buschain-mixer-gtk`).

| Path | Role |
|------|------|
| [`legacy/`](legacy/) | Python + CSS sources |
| [`buschain-mixer-gtk`](buschain-mixer-gtk) | Local/dev launcher (`nix develop` PATH) |
| [`../nix/gtk-mixer.nix`](../nix/gtk-mixer.nix) | Packaged wrapped binary |

Opt out: `BUSCHAIN_CONTROL_USE_GTK_MIXER=0`.

Popup order (tray / `buschain-ctl popup` / `buschain-waybar popup` → ctl):

1. Quickshell — only if `BUSCHAIN_CONTROL_QS_MIXER=1`
2. GTK layer-shell panel
3. egui — `buschain-control --popup`

Waybar should call `buschain-waybar popup`, which prefers **ctl → tray router**
so Hyprland’s bare PATH still gets a working panel. See
[`docs/HANDOVER-GTK-WAYBAR.md`](../../docs/HANDOVER-GTK-WAYBAR.md).
