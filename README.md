# BusChain Control

Linux PipeWire mixer: tracks + inserts, tray-resident, named sessions, and
Master HW volume controls for the desktop shell.

Standalone Nix flake project — build and run from this repository.

**New here?** Start with [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and [`.cursor/rules/live-graph.mdc`](.cursor/rules/live-graph.mdc).

## UI surfaces

| Surface | Role |
|---------|------|
| **Quickshell mixer** | Optional primary panel — bar volume pill + HW/apps/tracks/favorites |
| **egui `--popup`** | Portable fallback (tray “Mixer popup”, non-QS desktops) |
| **Full egui window** | Full mixer app (Show from tray) |
| **GTK overlay** | Deprecated — `buschain-mixer-gtk` only with `BUSCHAIN_CONTROL_USE_GTK_MIXER=1` |

## Binaries

| Binary | Role |
|--------|------|
| `buschain-control` | Owns the graph (in-process worker) + embeds IPC + tray UI. Close **hides**; Quit from tray / Settings. |
| `buschain-ctl` | CLI for shells / scripts (talks to embedded socket) |
| `buschain-waybar` | Thin helper (HW scroll uses direct `pactl`; status via ctl; also used by Quickshell) |
| `buschain-daemon` | Legacy headless supervisor (debug only — do not enable as a user unit) |
| `buschain-mixer-gtk` | Legacy GTK overlay (not installed on the hot path) |

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

Production and `./scripts/dev` use the same model: one process, in-process worker.

```bash
nix develop
./scripts/dev          # stop leftover daemon → cargo run
# or: make run
```

| Command | What it does |
|---------|----------------|
| `./scripts/dev` / `make run` | **Recommended** — tray UI + graph + embedded IPC |
| `./scripts/dev daemon` | Optional legacy headless daemon |
| `./scripts/dev stop` | Kill leftover headless daemons |

Flags: `--hidden` · `--popup` · `--daemon-client` (debug thin client). Plugin paths via `LADSPA_PATH` → `plugins/*/build`.

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

## Nix flake

```bash
nix build .#buschain-control
nix run .#buschain-control -- --hidden
nix run .#ctl -- status
```

Home Manager (packages only — start from the compositor `exec-once`, not systemd):

```nix
# flake inputs: buschain-control.url = "github:teoscloud/buschain-control";
imports = [ inputs.buschain-control.homeModules.buschain-control ];
services.buschain-control.enable = true;
```

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
