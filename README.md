# BusChain Control

<p align="center">
  <img src="docs/assets/hero.jpg" alt="BusChain Control — mixer, insert FX, and spectrum view" width="960" />
</p>

BusChain Control is a **PipeWire system mixer** for Linux. It aims for **DAW-grade control of the desktop audio graph** — tracks, insert FX, app routing, and sessions — without treating system sound as a second-class panel.

Route apps onto buses, stack builtins and LADSPA / LV2 / CLAP / VST3, expose virtual system I/O, drive Master HW from your bar, and save named layouts. Closing the window minimizes to the tray; right-click the icon → **Open BusChain Control** for the full mixer. Quit from the tray (or Settings) tears the graph down and restores hardware audio.

**TLDR:** mixer tracks · playback / capture racks · sealed wet FX · sticky virtual defaults · MIDI learn · Waybar / Quickshell / GTK shell hooks. Working daily driver on PipeWire (Hyprland primary); host APIs and crash-restore still deepening. Details: [`docs/TECHNICAL.md`](docs/TECHNICAL.md).

---

## Who it’s for

- Linux users on **PipeWire** who want a real mixer for the desktop, not just a volume slider
- Hyprland / Wayland rices (Quickshell, Waybar) and general desktops via GTK / egui popup
- Anyone routing browsers, games, Discord, mics, and hardware through per-app buses with FX

**Needs:** PipeWire (+ `pactl`), Wayland session. Optional GTK mixer strip via Nix / package deps.

---

## Install

No official Arch/Debian package yet — use Nix or build from source.

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

- Closing the window minimizes to the tray.
- Full mixer: right-click tray → **Open BusChain Control**. Left-click opens the compact popup.
- Quit from the tray (or Settings) stops audio ownership and restores HW default.
- CLI: `buschain-ctl status`

Disable any old user systemd unit if you used one before:

```bash
systemctl --user disable --now buschain-control 2>/dev/null || true
```

---

## Usage

Start with `buschain-control --hidden` for autostart, or open normally and close to tray.

| Action | Behavior |
|--------|----------|
| Left-click tray | Compact mixer popup (**QS → GTK → egui**) |
| Right-click → **Open BusChain Control** | Full mixer window |
| Tray → Quit | Tear down graph + restore HW audio |
| `buschain-ctl popup` / Waybar click | Same compact popup router |

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

**GTK layer-shell** compact mixer when packaged; egui `--popup` as fallback.

```bash
export BUSCHAIN_CONTROL_USE_GTK_MIXER=0   # force egui popup
export BUSCHAIN_CONTROL_SCROLL_STRIP=1    # opt-in GTK Master HW strip
```

### Day-to-day

| Action | How |
|--------|-----|
| Compact mixer | Left-click tray or Waybar pill |
| Full mixer | Right-click → Open BusChain Control |
| Assign app → track | Playback tab or drag onto channel rack |
| Virtual system out | Track → Create system virtual output |
| Add FX | Channel rack → Add plugin |
| Save layout | Sessions → Save / Save as… |
| Quit | Tray menu or Settings |

### Tips

- **Plugins:** VST3 under `~/.vst3` (or `VST3_PATH`); CLAP under `~/.clap`; LV2 via `LV2_PATH`. Restart the app after installing new plugins.
- **Sticky default:** set **System default** on a virtual track once — after Quit/reopen, BusChain reasserts that sink and reclaims playback apps.
- **Hollow desktop audio:** `buschain-ctl recover-audio`, or `systemctl --user restart wireplumber` as a blunt recovery.

---

## Features

### Mixer & graph

- Dynamic **tracks / buses** with mute, solo, listen, and gain
- **Playback** rack — pin apps onto tracks; reclaim onto sticky preferred default after restart
- **Input** rack — multi HW capture, shared with the desktop / other tracks
- **Virtual system output / input** — expose tracks as sinks or post-FX capture sources
- Sealed wet path: `{bus}.monitor → buschain_fx_* → buschain_post_* → hardware / Master`
- Live hotplug and warm adopt when the graph already matches the session

### Insert FX

- **BusChain builtins** — EQ, Theatre Drive, Room, Soft Clipper, limiter, denoiser, pitch, and more
- **Hosted formats** — LADSPA, LV2, CLAP, VST3 (in-process DSP; optional SHM sandbox)
- Per-insert power, wet Mix, reorder; egui param UIs; native VST3 editors via `buschain-plugin-surface`

### Sessions, MIDI & shell

- Named sessions under `~/.config/buschain-control/` with soft HW rebind
- MIDI devices, routes, and CC learn → insert parameters
- Waybar pill; compact popup order Quickshell → GTK → egui
- Tabs: Mixer · Playback · Recording · Output · Input · MIDI · Sessions · Settings

### Project state

| Area | Status |
|------|--------|
| Mixer, routing, virtual I/O, sessions | Stable |
| Built-in + hosted inserts / VST3 editors | Usable daily; host APIs still deepening |
| Compact popup (QS / GTK / egui) | Stable |
| Distro packages (Arch/Debian repos) | Not yet — Nix or source |
| Crash / `kill -9` audio restore | Still open (Quit path is solid) |

---

## Docs

| Doc | Audience |
|-----|----------|
| [`docs/TECHNICAL.md`](docs/TECHNICAL.md) | Features detail, binaries, Waybar, env reference |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Live graph / engine contract |
| [`docs/HANDOVER-QUICKSHELL.md`](docs/HANDOVER-QUICKSHELL.md) | Quickshell rice contract |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | Done / planned work |

---

## License

MIT — see [`LICENSE`](LICENSE).
