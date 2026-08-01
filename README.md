# BusChain Control

<p align="center">
  <img src="docs/assets/hero.jpg" alt="BusChain Control — mixer, insert FX, and spectrum view" width="960" />
</p>

**Tray-resident PipeWire system mixer for Linux.** Route desktop apps onto tracks, stack insert FX (builtins + LADSPA / LV2 / CLAP / VST3), drive Master HW from your bar, and save named sessions — without a separate audio daemon.

Close the window to **hide to tray**. Quit from the tray (or Settings) to tear the graph down and restore desktop audio.

---

## Project state

BusChain Control is a **working daily driver** on PipeWire (Hyprland / Wayland primary). The graph is owned **in-process** by the tray app: dual-lane control plane (interactive / supervisor / observer), sealed wet FX path, sticky virtual system defaults with reclaim on reopen, and IPC for `ctl` / Waybar / Quickshell / GTK.

| Area | Status |
|------|--------|
| Mixer tracks, gain / mute / solo / listen | Stable |
| App → track routing + playback reclaim | Stable |
| Virtual system output / input sinks | Stable |
| Built-in DSP + LADSPA / LV2 / CLAP / VST3 inserts | Usable daily; host APIs still deepening |
| Native VST3 floating editors (surface helper) | Working (XWayland float on Hyprland) |
| Sessions + soft HW rebind | Stable |
| Tray + QS / GTK / egui popup stack | Stable |
| MIDI learn → insert params | Working |
| Distro packages (Arch/Debian repos) | Not yet — Nix flake or build from source |
| Crash / `kill -9` audio restore | Still open (Quit path is solid) |

Deep reference: [`docs/TECHNICAL.md`](docs/TECHNICAL.md) · architecture: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) · roadmap: [`docs/ROADMAP.md`](docs/ROADMAP.md)

---

## Features

### Mixer & graph

- Dynamic **tracks / buses** with mute, solo, listen, and gain
- **Playback** rack — pin apps onto tracks; reclaim onto sticky preferred default after restart
- **Input** rack — multi HW capture, shared with the desktop / other tracks
- **Virtual system output** — expose a track as a PipeWire sink other apps can target
- **Virtual system input** — post-FX track as a capture source (`buschain_vin_*`)
- Sealed wet path: `{bus}.monitor → buschain_fx_* → buschain_post_* → hardware / Master`
- Live hotplug: fader / Props without tearing the graph; structural FX rewire when needed
- Warm adopt on restart when the live graph already matches the session

### Insert FX

- **BusChain builtins** — EQ, Theatre Drive, Room, Soft Clipper, limiter, denoiser, pitch, and more
- **Hosted formats** — LADSPA, LV2, CLAP, VST3 (discovery + in-process DSP; optional SHM sandbox)
- Per-insert power, wet Mix, reorder; egui param UIs and opt-in converted VST3 knobs
- **Native VST3 editors** via `buschain-plugin-surface` (same-instance meters)
- 3D / phosphor transfer views for dynamics (Soft Clipper, Limiter)

### Sessions & MIDI

- Named sessions under `~/.config/buschain-control/`
- Soft-bind on load: missing HW rebound by description; mix + FX kept
- Sticky `preferred_default_sink` across Quit (live default restored to HW; reopen reasserts BusChain)
- MIDI devices, routes, and CC learn → insert parameters (session-persisted)

### Desktop shell

- StatusNotifier tray — left-click compact mixer; right-click Open / Hide / Quit
- Popup order: **Quickshell → GTK layer-shell → egui `--popup`**
- Optional Waybar volume pill + Master HW scroll strip (Quickshell or opt-in GTK)
- CLI: `buschain-ctl` · helper: `buschain-waybar`

### App tabs

Mixer · Playback · Recording · Output · Input · MIDI · Sessions · Settings

---

## Requirements

- Linux with **PipeWire** (`pactl` / Pulse compatibility available)
- Wayland session for the tray UI (Hyprland is the primary target)
- Optional GTK mixer / scroll strip: GTK 3 + gtk-layer-shell + PyGObject (via Nix package / `nix develop`)

---

## Install

No official Arch/Debian package yet. Use Nix or build from source.

| Distro | Recommended |
|--------|-------------|
| **NixOS** | Flake package + optional Home Manager module |
| **Arch / Debian / Ubuntu** | [Nix](https://nixos.org/download/) profile install, or build from source |

### NixOS

```nix
# flake.nix inputs
{
  inputs.buschain-control.url = "github:teoscloud/buschain-control";
}
```

```nix
environment.systemPackages = [
  inputs.buschain-control.packages.${pkgs.system}.buschain-control
];
# or Home Manager:
# imports = [ inputs.buschain-control.homeModules.buschain-control ];
# services.buschain-control.enable = true;
```

One-shot:

```bash
nix profile install github:teoscloud/buschain-control
```

### Arch Linux

**Nix (simplest binaries)**

```bash
nix profile install github:teoscloud/buschain-control
```

**From source**

```bash
sudo pacman -S --needed rust cargo pkgconf openssl pipewire \
  libpulse gtk3 gtk-layer-shell python-gobject gobject-introspection \
  alsa-lib freetype2 cairo curl

git clone https://github.com/teoscloud/buschain-control.git
cd buschain-control
make plugins
cargo build --release -p buschain-control -p buschain-tools

mkdir -p ~/.local/bin
cp target/release/buschain-control target/release/buschain-ctl ~/.local/bin/
cp packaging/waybar/buschain-waybar packaging/mixer/buschain-mixer-gtk \
  packaging/scroll-strip/buschain-scroll-strip ~/.local/bin/
chmod +x ~/.local/bin/buschain-*
```

### Debian / Ubuntu

**Nix**

```bash
nix profile install github:teoscloud/buschain-control
```

**From source**

```bash
sudo apt update
sudo apt install -y build-essential pkg-config libssl-dev \
  libpipewire-0.3-dev libpulse-dev libgtk-3-dev libgtk-layer-shell-dev \
  python3-gi gir1.2-gtk-3.0 gir1.2-gtklayershell-0.1 \
  libasound2-dev libfreetype6-dev libcairo2-dev libcurl4-openssl-dev

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

git clone https://github.com/teoscloud/buschain-control.git
cd buschain-control
make plugins
cargo build --release -p buschain-control -p buschain-tools

mkdir -p ~/.local/bin
cp target/release/buschain-control target/release/buschain-ctl ~/.local/bin/
cp packaging/waybar/buschain-waybar packaging/mixer/buschain-mixer-gtk \
  packaging/scroll-strip/buschain-scroll-strip ~/.local/bin/
chmod +x ~/.local/bin/buschain-*
```

Use a current PipeWire session (`pipewire` + `pipewire-pulse`).

### From this checkout

```bash
cd buschain-control
nix develop
cargo run -- --hidden    # tray + graph + IPC
```

---

## First run

```bash
buschain-control --hidden
```

- Tray icon appears — **Show** / right-click opens the full mixer.
- Closing the window **hides**; tray → **Quit** stops audio ownership and restores HW default.
- CLI: `buschain-ctl status`

Disable any old user systemd unit if you used one before:

```bash
systemctl --user disable --now buschain-control 2>/dev/null || true
```

---

## Usage — tray first

Recommend `buschain-control --hidden` everywhere: left-click the tray for the compact mixer; right-click → **Open BusChain Control** for the full window.

| Action | Behavior |
|--------|----------|
| Tray left-click | Toggle mixer popup (**QS → GTK → egui**) |
| Tray right-click | Open full app · Hide · Quit |
| `buschain-ctl popup` / Waybar click | Same router |

### Hyprland + Quickshell

```conf
# ~/.config/hypr/hyprland.conf
exec-once = buschain-control --hidden
env = BUSCHAIN_CONTROL_QS_MIXER,1
env = BUSCHAIN_CONTROL_QS_STRIP,1
```

Waybar pill: merge [`packaging/waybar/module.jsonc`](packaging/waybar/module.jsonc) — **no** `on-scroll-*` (scroll belongs to the QS/GTK strip).

Quickshell contract: [`docs/HANDOVER-QUICKSHELL.md`](docs/HANDOVER-QUICKSHELL.md) · stubs in [`packaging/quickshell/`](packaging/quickshell/).

### General desktop (no Quickshell)

Tray + **GTK layer-shell** mixer when packaged; egui `--popup` as fallback.

```bash
export BUSCHAIN_CONTROL_USE_GTK_MIXER=0   # force egui popup
export BUSCHAIN_CONTROL_SCROLL_STRIP=1    # opt-in GTK Master HW strip
```

### Day-to-day

| Action | How |
|--------|-----|
| Compact mixer | Tray left-click or Waybar pill |
| Full mixer | Tray → Open BusChain Control |
| Assign app → track | Playback tab or drag onto channel rack |
| Virtual system out | Track → Create system virtual output |
| Add FX | Channel rack → Add plugin |
| Save layout | Sessions → Save / Save as… |
| Quit | Tray → Quit |

---

## Everyday tips

- **Plugins:** VST3 under `~/.vst3` (or `VST3_PATH`); CLAP under `~/.clap`; LV2 via `LV2_PATH`. Restart the tray after installing new plugins.
- **VST3 editors:** open from insert chrome; on Hyprland they float automatically when possible.
- **Sticky default:** set **System default** on a virtual track once — after Quit/reopen, BusChain reasserts that sink and reclaims playback apps (no need to click again).
- **Offline Waybar pill:** start `buschain-control --hidden` and restart Waybar if needed.
- **Hollow desktop audio:** `buschain-ctl recover-audio`, or `systemctl --user restart wireplumber` as a blunt recovery.

---

## Docs

| Doc | Audience |
|-----|----------|
| [`docs/TECHNICAL.md`](docs/TECHNICAL.md) | Features detail, binaries, Waybar, env reference, architecture notes |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Live graph / engine contract |
| [`docs/HANDOVER-QUICKSHELL.md`](docs/HANDOVER-QUICKSHELL.md) | Quickshell rice contract |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | Done / planned work |

---

## License

MIT — see [`LICENSE`](LICENSE).
