# BusChain scroll strip

Always-on transparent **GtkLayerShell** hit target that owns Master HW
hover-scroll. Waybar custom `on-scroll-*` cannot do 1:1 notches (SMOOTH
magnitude is discarded after one forkExec).

## Lifecycle

Started by the tray (`buschain-control --hidden`) when IPC comes up.
Disable: `BUSCHAIN_CONTROL_SCROLL_STRIP=0`.

## Geometry

Place the strip over your Waybar volume pill:

| Env | Default | Meaning |
|-----|---------|---------|
| `BUSCHAIN_CONTROL_SCROLL_ANCHOR` | `left` | `left` or `right` |
| `BUSCHAIN_CONTROL_SCROLL_MARGIN_TOP` | `0` | px from top (strip uses exclusive_zone=-1 so it sits ON the bar) |
| `BUSCHAIN_CONTROL_SCROLL_MARGIN_X` | `8` | px from left/right |
| `BUSCHAIN_CONTROL_SCROLL_WIDTH` | `110` | hit width |
| `BUSCHAIN_CONTROL_SCROLL_HEIGHT` | `38` | hit height (~bar) |

The strip sets layer-shell `exclusive_zone=-1` so Hyprland/Sway do **not**
push it below Waybar’s reserved zone. If scroll still misses, raise
`MARGIN_X` / `WIDTH` to match your pill (put the vol module at a fixed edge).

Click on the strip opens the mixer popup (same as waybar on-click).
