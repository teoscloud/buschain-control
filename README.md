# BusChain Control

Linux PipeWire mixer: tracks + inserts, tray-resident, named sessions, and
Master HW volume controls for the desktop shell.

Standalone Nix flake project — build and run from this repository.

**New here?** Start with [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and [`.cursor/rules/live-graph.mdc`](.cursor/rules/live-graph.mdc).

**GTK mixer + Waybar helper:** setup guide in [Waybar + mixer popup](#waybar--mixer-popup); internals in [`docs/HANDOVER-GTK-WAYBAR.md`](docs/HANDOVER-GTK-WAYBAR.md).

## UI surfaces

| Surface | Role |
|---------|------|
| **GTK layer-shell panel** | Default waybar / tray popup (`buschain-mixer-gtk`) |
| **egui `--popup`** | Fallback if GTK mixer is unavailable / disabled |
| **Full egui window** | Full mixer app (Show from tray) |
| **Quickshell mixer** | Optional — set `BUSCHAIN_CONTROL_QS_MIXER=1` |

## Binaries

| Binary | Role |
|--------|------|
| `buschain-control` | Owns the graph (in-process worker) + embeds IPC + tray UI. Close **hides**; Quit from tray / Settings. |
| `buschain-ctl` | CLI for shells / scripts (talks to embedded socket) |
| `buschain-waybar` | Waybar helper (status / scroll / popup) — packaged with absolute-path module |
| `buschain-mixer-gtk` | GTK layer-shell mixer panel (shipped in the same package) |
| `buschain-daemon` | Legacy headless supervisor (debug only — do not enable as a user unit) |

IPC: `$XDG_RUNTIME_DIR/buschain-control/daemon.sock` (newline JSON), served by the tray app.

## Session autostart

```bash
# Hyprland example (tray-resident)
exec-once = buschain-control --hidden
```

Set `BUSCHAIN_CONTROL_QS_MIXER=1` so tray/ctl popup prefers a Quickshell mixer panel
(`qs ipc call mixer toggle`) when available.

After switching from an old systemd unit:

```bash
systemctl --user disable --now buschain-control
```

## Local development

One command. Debug `cargo run` bootstraps plugins, builds `buschain-ctl` for
waybar, clears leftover headless daemons, then starts the tray app.

```bash
nix develop
cargo run                 # full local stack
cargo run -- --hidden     # tray only (autostart-style)
cargo ctl -- status       # talk to the running app
```

| Command | What it does |
|---------|----------------|
| `cargo run` | **Recommended** — plugins + ctl + tray UI + graph + IPC |
| `cargo run -- --hidden` | Same, start withdrawn |
| `cargo ctl -- …` | `buschain-ctl` against the live socket |
| `./scripts/dev stop` | Kill leftover headless daemons |

Flags: `--hidden` · `--popup` · `--daemon-client` (debug thin client). Set
`BUSCHAIN_CONTROL_SKIP_BOOTSTRAP=1` to skip the local bootstrap (packaged runs).

## Sessions

- Library: `~/.config/buschain-control/sessions/<slug>.json`
- Active slug: `~/.config/buschain-control/active`
- Legacy `session.json` migrates once → `sessions/default.json`
- Settings → Session: Save / Save as… / Load / Delete
- **Soft bind** on load: missing Master HW / mics rebound by description or cleared; mix + FX kept
- Mixer favorites: `~/.config/buschain-control/mixer-pins.json` (QS + egui popup + legacy GTK)

## Master HW volume

Scroll helpers and status **hard-clamp at 100%** (no boost in the label). If Pulse
reports >100%, the waybar/QS helper pulls the sink back to 100%.

```bash
buschain-waybar status|up|down|popup
buschain-ctl hw-vol get|set <pct>|up|down|mute toggle
```

## Waybar + mixer popup

BusChain owns the helper, tray IPC, and GTK panel. **Your rice owns the Waybar
module entry and CSS** — this project never writes `~/.config/waybar`. Snippets
live in [`packaging/waybar/`](packaging/waybar/). Internals:
[`docs/HANDOVER-GTK-WAYBAR.md`](docs/HANDOVER-GTK-WAYBAR.md).

### What you get

| Action | Behavior |
|--------|----------|
| Status pill | Master HW volume (`vol 42` / muted / offline) every ~3s |
| Scroll | ±5% Master HW volume (hard-capped at 100%) |
| Click | Mixer **popup** — GTK layer-shell panel by default, egui fallback |

Click path: `buschain-waybar popup` → `buschain-ctl popup` → tray
`spawn_mixer_popup()` → GTK (`buschain-mixer-gtk`) → egui `--popup`. Prefer the
**ctl → tray** route so Waybar’s thin PATH does not need PyGObject.

### Checklist

1. Tray running (`buschain-control --hidden` or `cargo run -- --hidden`).
2. `custom/buschain-control` in `modules-left` or `modules-right`.
3. Module `exec` / `on-click` / scroll call `buschain-waybar` (PATH **or** absolute).
4. Pill CSS classes: `online` · `muted` · `offline`.
5. Restart Waybar after editing config/CSS.

### Module (waybar `config`)

Add the module id to a modules list, then define it:

```jsonc
"modules-left": [
  "hyprland/window",
  "custom/buschain-control"
],

"custom/buschain-control": {
  "format": "{}",
  "return-type": "json",
  "exec": "buschain-waybar status",
  "interval": 3,
  "signal": 9,
  "exec-on-event": true,
  "on-click": "buschain-waybar popup",
  "on-scroll-up": "buschain-waybar up",
  "on-scroll-down": "buschain-waybar down",
  "tooltip": true
}
```

Same object: [`packaging/waybar/module.jsonc`](packaging/waybar/module.jsonc).

#### Packaged / on PATH (recommended)

```bash
nix profile install github:teoscloud/buschain-control
# or Home Manager module below — then `buschain-waybar` is on PATH
```

Use the short names above. If Waybar was started before the profile was on
PATH, restart Waybar (or use store-absolute paths from Home Manager).

#### Git checkout (no install)

Waybar often has a bare session PATH. Point every hook at the helper script:

```jsonc
"custom/buschain-control": {
  "format": "{}",
  "return-type": "json",
  "exec": "<checkout>/packaging/waybar/buschain-waybar status",
  "interval": 3,
  "signal": 9,
  "exec-on-event": true,
  "on-click": "<checkout>/packaging/waybar/buschain-waybar popup",
  "on-scroll-up": "<checkout>/packaging/waybar/buschain-waybar up",
  "on-scroll-down": "<checkout>/packaging/waybar/buschain-waybar down",
  "tooltip": true
}
```

Replace `<checkout>` with your clone (e.g. `$HOME/src/buschain-control`). Run the
tray from `nix develop` + `cargo run -- --hidden` so `buschain-ctl` and the
nix-wrapped `buschain-mixer-gtk` exist. The helper also looks for
`<checkout>/target/debug/buschain-ctl`.

### Style (waybar `style.css`)

Pill styling matches the frosted GTK mixer (`#f7f5ff` on dark glass). Paste as-is
or restyle; keep the selector and class names.

```css
#custom-buschain-control {
  font-weight: bold;
  margin: 4px 0px;
  margin-left: 7px;
  padding: 0px 16px;
  background: rgba(24, 25, 29, 0.5);
  color: #f7f5ff;
  border-radius: 24px 10px 24px 10px;
}
#custom-buschain-control:hover {
  background: rgba(42, 44, 46, 0.55);
  color: #f7f5ff;
}
#custom-buschain-control.offline {
  background: rgba(24, 25, 29, 0.35);
  color: rgba(247, 245, 255, 0.45);
}
#custom-buschain-control.muted {
  color: rgba(247, 245, 255, 0.50);
}
```

Same file: [`packaging/waybar/style.css`](packaging/waybar/style.css).

GTK panel chrome (not Waybar) ships with the package at
`share/buschain-mixer/style.css` / `packaging/mixer/legacy/style.css`.

### Autostart (Hyprland)

```conf
exec-once = buschain-control --hidden
exec-once = waybar
```

Order does not matter much; the pill shows `vol —` / class `offline` until the
tray owns `$XDG_RUNTIME_DIR/buschain-control/daemon.sock`.

### Home Manager (bins only — you still own Waybar)

BusChain’s home module puts packages on PATH and enables the GTK mixer env. It
**does not** write Waybar config (by design — rice stays yours).

```nix
# flake inputs
{
  inputs.buschain-control.url = "github:teoscloud/buschain-control";
  # ...
}

# home.nix (or host home module)
{
  imports = [ inputs.buschain-control.homeModules.buschain-control ];

  services.buschain-control.enable = true;

  # Option A — manage Waybar yourself (dotfiles / home.file), use PATH names:
  #   exec = "buschain-waybar status";
  #   on-click = "buschain-waybar popup";
  #
  # Option B — absolute store paths (survives Waybar’s thin PATH):
  # let bc = inputs.buschain-control.packages.${pkgs.system}.buschain-control; in
  #   exec = "${bc}/bin/buschain-waybar status";
  #   on-click = "${bc}/bin/buschain-waybar popup";
  #   on-scroll-up = "${bc}/bin/buschain-waybar up";
  #   on-scroll-down = "${bc}/bin/buschain-waybar down";

  # Example with programs.waybar (settings shape varies by HM version):
  # programs.waybar.enable = true;
  # programs.waybar.settings.mainBar = {
  #   modules-left = [ "hyprland/window" "custom/buschain-control" ];
  #   "custom/buschain-control" = { ... };  # see Module section
  # };
  # programs.waybar.style = builtins.readFile
  #   "${inputs.buschain-control}/packaging/waybar/style.css";
}
```

Reference snippets are also installed into the package at
`$out/share/buschain-control/waybar/{module.jsonc,style.css}`.

### Popup backends

| Priority | Surface | Enable |
|----------|---------|--------|
| 1 | Quickshell mixer | `BUSCHAIN_CONTROL_QS_MIXER=1` only |
| 2 | GTK layer-shell | default when `buschain-mixer-gtk` is available |
| 3 | egui `--popup` | fallback |

Opt out of GTK: `BUSCHAIN_CONTROL_USE_GTK_MIXER=0`. Override mixer binary:
`BUSCHAIN_CONTROL_MIXER=/path/to/buschain-mixer-gtk`.

### Troubleshooting

| Symptom | Fix |
|---------|-----|
| `vol —` / class `offline` | Start tray; check socket exists |
| Click does nothing | Ensure `on-click` → `buschain-waybar popup` and tray is running |
| Click OK, no panel | Packaged/`nix develop` mixer (system python often lacks `gi`) |
| Scroll works, status stale | `exec-on-event: true`; restart Waybar |
| Helper not found | PATH install, or absolute path to `packaging/waybar/buschain-waybar` |

```bash
buschain-waybar status          # JSON for the pill
buschain-waybar popup           # same as click
buschain-ctl status             # tray / IPC alive?
command -v buschain-mixer-gtk   # GTK panel present?
```

## Nix flake

```bash
nix build .#buschain-control
nix run .#buschain-control -- --hidden
nix run .#ctl -- status
```

Home Manager module (bins on PATH — **not** Waybar config): see [Waybar + mixer popup](#waybar--mixer-popup) above.

## Architecture

- `engine/` — PipeWire intents, GraphClock, FX, warm-adopt / sealed arm
- `app/` — tray UI + in-process worker + embedded IPC + session store
- `control/` — `buschain-ctl` (+ legacy `buschain-daemon`)
- Warm restart: if the live graph already matches the session, adopt (no FX ForceRespawn)

Details: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), [`engine/README.md`](engine/README.md).

## Tabs

Mixer · Playback · Recording · Devices · Settings

## License

MIT — see [`LICENSE`](LICENSE).
