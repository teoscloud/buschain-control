# Quickshell bridge stubs (unstyled)

BusChain provides the **daemon + ctl contract**. Quant (rice flake) styles and
owns the real Quickshell panels. These files are a drop-in starting point.

| File | Role |
|------|------|
| [`qs-mixer-toggle.sh`](qs-mixer-toggle.sh) | Install to `~/.config/quickshell/scripts/qs-mixer-toggle.sh` |
| [`mixer.qml`](mixer.qml) | Skeleton PanelWindow + poll/mutate via `buschain-ctl` |
| [`scroll-strip.qml`](scroll-strip.qml) | Skeleton Master HW hit target |

Full contract: [`docs/HANDOVER-QUICKSHELL.md`](../../docs/HANDOVER-QUICKSHELL.md).

## Enable from BusChain tray

```bash
export BUSCHAIN_CONTROL_QS_MIXER=1
export BUSCHAIN_CONTROL_QS_STRIP=1   # skip GTK scroll strip
# optional hard-disable GTK strip forever:
# export BUSCHAIN_CONTROL_SCROLL_STRIP=0
```

Popup order: QS (script / `qs ipc`) → GTK fallback → egui.
