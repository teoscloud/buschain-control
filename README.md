# BusChain Control

Linux **PipeWire system mixer**: dynamic tracks, insert FX, named sessions, and
Master HW volume for the desktop shell. One tray-resident process owns the
audio graph; close **hides**, Quit tears it down.

Standalone Nix flake — build and run from this repository.

**Dig deeper:** [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) ·
[`docs/HANDOVER-GTK-WAYBAR.md`](docs/HANDOVER-GTK-WAYBAR.md) ·
[`.cursor/rules/live-graph.mdc`](.cursor/rules/live-graph.mdc)

---

## Features

### Mixer & graph

- Dynamic **tracks / buses** with mute, solo, listen, and gain
- **Channel rack** of inserts per track (add / remove / reorder / power / wet Mix)
- Sealed wet path: `{bus}.monitor → buschain_fx_* → buschain_post_* → dest`
- Live hotplug: knob Props without tearing the graph; structural FX rewire when needed
- Warm adopt on restart when the live graph already matches the session
- Master HW out clock / quantum (Settings → Audio) with soft-quantum option

### Plugin hosting

| Format | Discovery | Audio | Native GUI |
|--------|-----------|-------|------------|
| **LADSPA** | Scan + BusChain builtins (`plugins/*/build`) | In-process (primary path) | egui param UIs |
| **VST3** | Filesystem (`VST3_PATH`, `~/.vst3`, …) | In-process `Vst3Instance`; optional SHM sandbox | Floating editor via `buschain-plugin-surface` (same instance → **live meters**) |
| **CLAP** | Filesystem (`.clap`) | In-process `ClapInstance`; optional sandbox | egui dynamic params (no native editor yet) |
| **LV2** | Filesystem (`LV2_PATH`) | In-process `Lv2Instance` | egui dynamic params |

- Mixer **Add** lists LADSPA / CLAP / VST3 / LV2 (respects Config → Plugins toggles/paths)
- **Converted params**: opt-in egui knobs for VST3 when you want host-side controls
- **Sidechain source** picker in the plugin chrome (session + fingerprint; aux audio wiring still evolving)
- **Sandbox untrusted** (Settings → Plugins): per-track SHM child (`buschain-plugin-dsp`) via `BUSCHAIN_SANDBOX_ALL` / `BUSCHAIN_SANDBOX_PLUGINS`

### Native VST3 editors

- Opening a VST3 insert **promotes** that slot to `buschain-plugin-surface` (DSP + GUI in one helper process)
- Host UI defaults to **native Wayland** (`WINIT_UNIX_BACKEND=wayland`)
- Editors open as **floating XWayland** windows (no in-rect embed into egui)
- On Hyprland, BusChain asks `hyprctl` to **float** new editor windows so they don’t tile
- Kill-switch: `BUSCHAIN_VST3_SURFACE=0` → legacy `buschain-plugin-ui` (separate idle instance; meters stay dead)
- Closing the egui plugin chrome demotes the slot back toward in-process hosting after state harvest

### MIDI

- MIDI tab: device list, enable/routes, CC learn → insert parameters
- Session-persisted devices, routes, and CC maps

### Desktop shell

- Tray icon (Show / Hide / Quit)
- Waybar pill + scroll Master HW volume (hard-capped at 100%)
- Mixer popup: GTK layer-shell (default) → egui `--popup` fallback → optional Quickshell

### Sessions

- Named library under `~/.config/buschain-control/sessions/`
- Soft-bind on load: missing HW/mics rebound by description or cleared; mix + FX kept
- Mixer favorites: `mixer-pins.json`

---

## UI surfaces

| Surface | Role |
|---------|------|
| **Full egui window** | Mixer, Playback, Recording, Output/Input devices, MIDI, Settings |
| **GTK layer-shell panel** | Default waybar / tray mixer popup (`buschain-mixer-gtk`) |
| **egui `--popup`** | Fallback popup if GTK is unavailable / disabled |
| **Quickshell mixer** | Optional — `BUSCHAIN_CONTROL_QS_MIXER=1` |

---

## Binaries

| Binary | Role |
|--------|------|
| `buschain-control` | Graph + tray UI + embedded IPC. Close hides; Quit from tray / Settings. |
| `buschain-ctl` | CLI for shells / scripts (same unix socket) |
| `buschain-waybar` | Waybar helper: `status` / `up` / `down` / `popup` |
| `buschain-mixer-gtk` | GTK layer-shell mixer panel |
| `buschain-plugin-surface` | VST3 same-instance DSP + native editor (meters) |
| `buschain-plugin-dsp` | Headless per-track sandbox DSP (SHM) |
| `buschain-plugin-ui` | Legacy editor-only helper (fallback) |
| `buschain-daemon` | Legacy headless supervisor (**debug only** — do not enable as a user unit) |

IPC: `$XDG_RUNTIME_DIR/buschain-control/daemon.sock` (newline JSON), served by the tray app.

---

## Quick start

### Autostart (Hyprland)

```bash
exec-once = buschain-control --hidden
```

Disable any old user unit after switching:

```bash
systemctl --user disable --now buschain-control
```

### Local development

```bash
nix develop
cargo run                              # plugins + ctl + tray + graph + IPC
cargo run -- --hidden                  # tray-only (autostart-style)
cargo ctl -- status
cargo build -p buschain-tools \
  --bin buschain-plugin-surface \
  --bin buschain-plugin-dsp \
  --bin buschain-plugin-ui
```

| Command | What it does |
|---------|----------------|
| `cargo run` | Recommended full local stack (debug bootstrap) |
| `cargo run -- --hidden` | Same, start withdrawn |
| `cargo ctl -- …` | Talk to the running app |
| `./scripts/dev stop` | Kill leftover headless daemons |

Flags: `--hidden` · `--popup` · `--daemon-client` (debug thin client).  
Skip bootstrap: `BUSCHAIN_CONTROL_SKIP_BOOTSTRAP=1`.

`nix develop` sets plugin search paths, puts `target/debug` on `PATH`, and a broad
`LD_LIBRARY_PATH` so commercial `.vst3` modules can `dlopen` usual system libs
(freetype, cairo, curl, alsa, sndfile, X11/GL, gtk3, …). Packaged wraps use the same set.

### Packaged

```bash
nix build .#buschain-control
nix run .#buschain-control -- --hidden
nix run .#ctl -- status
nix profile install github:teoscloud/buschain-control   # optional
```

---

## Plugin scan roots

| Format | Typical roots |
|--------|----------------|
| LADSPA | `LADSPA_PATH`, nix develop → `plugins/*/build` |
| LV2 | `LV2_PATH` |
| CLAP | `CLAP_PATH`, `~/.clap` |
| VST3 | `VST3_PATH`, `~/.vst3`, `~/.local/lib/vst3`, `/usr/lib/vst3` (recursive) |

If a VST3 fails with `lib….so: not found`, `ldd` the `.so` under
`Plugin.vst3/Contents/x86_64-linux/` and add that package to
`vst3PluginRuntimeLibs` in [`flake.nix`](flake.nix).

### Hyprland + floating VST3 editors

When `HYPRLAND_INSTANCE_SIGNATURE` is set, BusChain runs
`hyprctl dispatch setfloating` for new editor windows. Optional backup rule:

```conf
windowrulev2 = float, title:( - VST3)$
```

---

## Sessions & paths

| Path | Purpose |
|------|---------|
| `~/.config/buschain-control/sessions/<slug>.json` | Session library |
| `~/.config/buschain-control/active` | Active slug |
| `~/.config/buschain-control/session.json` | Legacy → migrates once to `sessions/default.json` |
| `~/.config/buschain-control/mixer-pins.json` | Mixer favorites |
| `$XDG_RUNTIME_DIR/buschain-control/daemon.sock` | Embedded IPC |

Settings → Session: Save / Save as… / Load / Delete.

---

## Master HW volume

Scroll helpers and status **hard-clamp at 100%**. If Pulse reports >100%, helpers pull the sink back.

```bash
buschain-waybar status|up|down|popup
buschain-ctl hw-vol get|set <pct>|up|down|mute toggle
```

---

## Waybar + mixer popup

BusChain owns the helper, tray IPC, and GTK panel. **Your rice owns the Waybar
module entry and CSS** — this project never writes `~/.config/waybar`. Snippets:
[`packaging/waybar/`](packaging/waybar/). Internals:
[`docs/HANDOVER-GTK-WAYBAR.md`](docs/HANDOVER-GTK-WAYBAR.md).

### Behavior

| Action | Behavior |
|--------|----------|
| Status pill | Master HW volume (`vol 42` / muted / offline) every ~3s |
| Scroll | ±5% Master HW volume (hard-capped at 100%) |
| Click | Mixer popup — GTK layer-shell by default, egui fallback |

Click path: `buschain-waybar popup` → `buschain-ctl popup` → tray → GTK → egui `--popup`.

### Checklist

1. Tray running (`buschain-control --hidden` or `cargo run -- --hidden`)
2. `custom/buschain-control` in a Waybar modules list
3. Module `exec` / `on-click` / scroll call `buschain-waybar` (PATH or absolute)
4. Pill CSS classes: `online` · `muted` · `offline`
5. Restart Waybar after editing config/CSS

### Module (waybar `config`)

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

#### Packaged / on PATH

```bash
nix profile install github:teoscloud/buschain-control
```

Use the short names above. Restart Waybar if it was started before the profile was on PATH.

#### Git checkout (no install)

Waybar often has a bare session PATH — point hooks at the helper script:

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

Run the tray from `nix develop` + `cargo run -- --hidden` so `buschain-ctl` and
the nix-wrapped `buschain-mixer-gtk` exist. The helper also looks for
`<checkout>/target/debug/buschain-ctl`.

### Style (waybar `style.css`)

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

### Autostart (Hyprland)

```conf
exec-once = buschain-control --hidden
exec-once = waybar
```

The pill shows offline until the tray owns the IPC socket.

### Home Manager (bins only — you still own Waybar)

```nix
{
  inputs.buschain-control.url = "github:teoscloud/buschain-control";
  # ...
}

{
  imports = [ inputs.buschain-control.homeModules.buschain-control ];
  services.buschain-control.enable = true;
  # Waybar config stays in your rice — use PATH names or store-absolute paths.
}
```

Reference snippets also ship at `$out/share/buschain-control/waybar/`.

### Popup backends

| Priority | Surface | Enable |
|----------|---------|--------|
| 1 | Quickshell mixer | `BUSCHAIN_CONTROL_QS_MIXER=1` |
| 2 | GTK layer-shell | default when `buschain-mixer-gtk` is available |
| 3 | egui `--popup` | fallback |

Opt out of GTK: `BUSCHAIN_CONTROL_USE_GTK_MIXER=0`.  
Override mixer binary: `BUSCHAIN_CONTROL_MIXER=/path/to/buschain-mixer-gtk`.

### Troubleshooting

| Symptom | Fix |
|---------|-----|
| `vol —` / class `offline` | Start tray; check socket exists |
| Click does nothing | `on-click` → `buschain-waybar popup` and tray running |
| Click OK, no panel | Use packaged/`nix develop` mixer (system python often lacks `gi`) |
| Scroll works, status stale | `exec-on-event: true`; restart Waybar |
| Helper not found | PATH install, or absolute path to `packaging/waybar/buschain-waybar` |
| VST3 editor tiled (Hyprland) | Ensure `hyprctl` works; add the optional `windowrulev2` above |
| VST3 missing `.so` | Expand `vst3PluginRuntimeLibs` in `flake.nix` |

```bash
buschain-waybar status
buschain-ctl status
command -v buschain-mixer-gtk
command -v buschain-plugin-surface
```

---

## Environment reference

| Variable | Role |
|----------|------|
| `WINIT_UNIX_BACKEND` | Host window system; default forced to `wayland` if unset |
| `BUSCHAIN_VST3_SURFACE` | `0` / `false` / `off` disables surface promote |
| `BUSCHAIN_SANDBOX_ALL` | Sandbox all non-trusted inserts |
| `BUSCHAIN_SANDBOX_PLUGINS` | CSV of plugin keys to sandbox |
| `BUSCHAIN_CONTROL_SKIP_BOOTSTRAP` | Skip debug bootstrap on `cargo run` |
| `BUSCHAIN_CONTROL_USE_GTK_MIXER` | `0` disables GTK popup |
| `BUSCHAIN_CONTROL_MIXER` | Override GTK mixer binary |
| `BUSCHAIN_CONTROL_QS_MIXER` | Prefer Quickshell mixer |
| `BUSCHAIN_CONTROL_CTL` | Path to `buschain-ctl` |
| `BUSCHAIN_CONTROL_DAEMON` | Socket path override |
| `BUSCHAIN_CONTROL_USE_DAEMON` | Thin-client debug |
| `VST3_PATH` / `CLAP_PATH` / `LV2_PATH` / `LADSPA_PATH` | Plugin scan roots |
| `HYPRLAND_INSTANCE_SIGNATURE` | Enables Hyprland float dispatch for editors |

---

## Architecture

One `buschain-control` process owns the graph (in-process worker + coalesce).
The engine insert rack (`PwFxNode` + `AudioProcessor` host) is the DSP path;
PipeWire is mixer I/O. Helpers (`plugin-surface` / `plugin-dsp`) run only when
promoted or sandboxed.

| Tree | Role |
|------|------|
| `engine/` | PipeWire intents, GraphClock, insert host (LADSPA/CLAP/VST3/LV2), MIDI, sandbox SHM |
| `app/` | Tray UI, session store, in-process worker, embedded IPC |
| `control/` | `buschain-ctl`, plugin helpers, legacy daemon |
| `plugins/` | BusChain LADSPA builtins |
| `packaging/` | Waybar / GTK mixer / desktop files |

Warm restart: if the live graph already matches the session, adopt (no FX ForceRespawn).

Details: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), [`engine/README.md`](engine/README.md).

---

## App tabs

Mixer · Playback · Recording · Output · Input · MIDI · Session · Settings

---

## License

MIT — see [`LICENSE`](LICENSE).
