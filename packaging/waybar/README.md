# Waybar module — status pill

Optional Waybar custom module: Master HW % via `buschain-ctl status`, click →
`buschain-waybar popup` (same router as tray: **QS → GTK → egui**).

| File | Role |
|------|------|
| [`module.jsonc`](module.jsonc) | Snippet to merge into your Waybar config |
| [`style.css`](style.css) | Optional pill CSS |
| [`buschain-waybar`](buschain-waybar) | Helper: `status` / `up` / `down` / `popup` |

Do **not** add Waybar `on-scroll-*` for Master HW — use QS strip or opt-in
`BUSCHAIN_CONTROL_SCROLL_STRIP=1` for the GTK strip.
