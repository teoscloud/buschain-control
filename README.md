# BusChain Control

A tray-resident **PipeWire system mixer** for Linux. Route apps onto tracks, stack insert FX, save sessions, and drive Master HW volume from your bar — without juggling a separate audio daemon.

Close the window to **hide to tray**. Quit from the tray (or Settings) to tear the graph down.

**Technical specs, binaries, env vars, and architecture:** [`docs/TECHNICAL.md`](docs/TECHNICAL.md)

---

## What you get

- **Mixer tracks** — assign apps, set gain / mute, listen, create virtual system outputs
- **Insert FX** — BusChain builtins (EQ, reverb, denoiser, limiter, …) plus LADSPA / LV2 / CLAP / VST3 discovery
- **Sessions** — named setups under `~/.config/buschain-control/`
- **Desktop shell** — Waybar pill, Master HW scroll, mixer popup (Quickshell preferred, GTK fallback, egui last)

---

## Requirements

- Linux with **PipeWire** (PulseAudio compatibility tools such as `pactl` available)
- A Wayland session for the tray UI (Hyprland is the primary target)
- For the GTK mixer popup: GTK 3 + gtk-layer-shell + PyGObject (included in the Nix package / `nix develop`)

---

## Install

There is no distro package in the official Arch/Debian repos yet. Pick one path:

| Distro | Recommended |
|--------|-------------|
| **NixOS** | Flake package + optional Home Manager module |
| **Arch / Debian / Ubuntu** | [Nix](https://nixos.org/download/) profile install, **or** build from source |

### NixOS

Add the flake and put the package on your system (or home) packages:

```nix
# flake.nix inputs
{
  inputs.buschain-control.url = "github:teoscloud/buschain-control";
}
```

```nix
# configuration or home.nix
environment.systemPackages = [
  inputs.buschain-control.packages.${pkgs.system}.buschain-control
];
# or, with Home Manager:
# imports = [ inputs.buschain-control.homeModules.buschain-control ];
# services.buschain-control.enable = true;
```

One-shot without wiring a flake:

```bash
nix profile install github:teoscloud/buschain-control
```

Rebuild / log in so `buschain-control`, `buschain-ctl`, and `buschain-waybar` are on `PATH`.

### Arch Linux

**Option A — Nix (simplest binaries)**

```bash
# Install Nix if needed: https://nixos.org/download/
nix profile install github:teoscloud/buschain-control
```

**Option B — Build from source**

```bash
sudo pacman -S --needed rust cargo pkgconf openssl pipewire \
  libpulse gtk3 gtk-layer-shell python-gobject gobject-introspection \
  alsa-lib freetype2 cairo curl

git clone https://github.com/teoscloud/buschain-control.git
cd buschain-control
make plugins
cargo build --release -p buschain-control -p buschain-tools

# Install bins somewhere on PATH, e.g.:
mkdir -p ~/.local/bin
cp target/release/buschain-control target/release/buschain-ctl ~/.local/bin/
cp packaging/waybar/buschain-waybar packaging/mixer/buschain-mixer-gtk \
  packaging/scroll-strip/buschain-scroll-strip ~/.local/bin/
chmod +x ~/.local/bin/buschain-*
```

Ensure PipeWire is your session audio stack (`systemctl --user status pipewire pipewire-pulse`).

### Debian / Ubuntu

**Option A — Nix**

```bash
nix profile install github:teoscloud/buschain-control
```

**Option B — Build from source**

```bash
sudo apt update
sudo apt install -y build-essential pkg-config libssl-dev \
  libpipewire-0.3-dev libpulse-dev libgtk-3-dev libgtk-layer-shell-dev \
  python3-gi gir1.2-gtk-3.0 gir1.2-gtklayershell-0.1 \
  libasound2-dev libfreetype6-dev libcairo2-dev libcurl4-openssl-dev

# Rust via rustup (recommended): https://rustup.rs/
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

Use a current PipeWire session (`pipewire` + `pipewire-pulse`). Older Ubuntu releases may need newer PipeWire from a PPA or distro upgrade.

### From this checkout (any distro with Nix)

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

- Tray icon appears (Show opens the full mixer).
- Closing the window **hides**; use tray → **Quit** to stop audio ownership.
- Optional CLI: `buschain-ctl status`

Disable any old user systemd unit if you used one before:

```bash
systemctl --user disable --now buschain-control 2>/dev/null || true
```

---

## Usage — Hyprland + Quickshell (recommended)

This is the primary desktop target: tray owns audio; Quickshell owns the styled mixer / scroll strip; Waybar shows a volume pill.

### 1. Autostart the tray

`~/.config/hypr/hyprland.conf`:

```conf
exec-once = buschain-control --hidden
```

For Quickshell as the mixer popup + Master HW strip (skip GTK strip):

```conf
env = BUSCHAIN_CONTROL_QS_MIXER,1
env = BUSCHAIN_CONTROL_QS_STRIP,1
```

Or export those in the environment that starts the tray.

### 2. Waybar pill

Merge [`packaging/waybar/module.jsonc`](packaging/waybar/module.jsonc) into your Waybar config and add `custom/buschain-control` to a modules list. **Do not** add `on-scroll-up` / `on-scroll-down` — scroll is owned by the Quickshell (or GTK) strip so wheel notches stay accurate.

```jsonc
"custom/buschain-control": {
  "format": "{}",
  "return-type": "json",
  "exec": "buschain-ctl status",
  "interval": 3,
  "signal": 9,
  "exec-on-event": false,
  "on-click": "buschain-waybar popup",
  "tooltip": true
}
```

Optional pill CSS: [`packaging/waybar/style.css`](packaging/waybar/style.css).

Restart Waybar after edits. The pill shows offline until the tray owns the socket.

### 3. Quickshell mixer

BusChain exposes the daemon contract (`buschain-ctl mixer`, popup, tick file). Your rice styles the panel.

1. Copy the toggle bridge:

```bash
mkdir -p ~/.config/quickshell/scripts
cp /path/to/buschain-control/packaging/quickshell/qs-mixer-toggle.sh \
  ~/.config/quickshell/scripts/
chmod +x ~/.config/quickshell/scripts/qs-mixer-toggle.sh
```

2. Implement / wire the panel from the stubs in [`packaging/quickshell/`](packaging/quickshell/) using the contract in [`docs/HANDOVER-QUICKSHELL.md`](docs/HANDOVER-QUICKSHELL.md).

Clicking the Waybar pill runs `buschain-waybar popup` → tray prefers Quickshell when `BUSCHAIN_CONTROL_QS_MIXER=1`, the toggle script exists, or `qs` is on `PATH`.

### 4. Day-to-day

| Action | How |
|--------|-----|
| Open full mixer | Tray → Show |
| Bar mixer popup | Click Waybar BusChain pill |
| Master HW volume | Scroll over the strip on the pill (not Waybar `on-scroll`) |
| Assign an app to a track | Playback tab → pick track / drag onto channel rack |
| Add FX | Channel rack → Add plugin |
| Save layout | Session tab → Save / Save as… |
| Quit | Tray → Quit |

---

## Usage — general desktop (Waybar + GTK)

Works without Quickshell: GTK layer-shell mixer popup and GTK Master HW scroll strip.

1. Start the tray: `buschain-control --hidden` (Hyprland `exec-once`, or your WM autostart).
2. Add the same Waybar module as above (**no** `on-scroll-*`).
3. Ensure `buschain-mixer-gtk` and `buschain-scroll-strip` are on `PATH` (Nix package includes them; from source, copy from `packaging/`).
4. Click the pill → GTK mixer. Scroll the strip over the pill → Master HW ±5% (capped at 100%).

Force GTK even if Quickshell is installed:

```bash
export BUSCHAIN_CONTROL_USE_GTK_MIXER=1
```

Disable GTK popup (egui `--popup` fallback only):

```bash
export BUSCHAIN_CONTROL_USE_GTK_MIXER=0
```

If the scroll strip doesn’t line up with your pill, adjust geometry env vars — see [`docs/TECHNICAL.md`](docs/TECHNICAL.md#master-hw-volume).

### Without Waybar

You can still use BusChain as a tray app only: Show the full egui window, use Settings / Output for Master HW, and ignore bar integration.

---

## Everyday tips

- **Plugins:** put VST3 under `~/.vst3` (or set `VST3_PATH`); CLAP under `~/.clap`; LV2 via `LV2_PATH`. Rebuild/restart the tray after installing new plugins.
- **VST3 editors:** open from the insert chrome; on Hyprland they are floated automatically when possible.
- **Virtual system output:** on a track, use **Create** (virtual output) so other apps can target that bus as a sink.
- **Offline pill:** tray isn’t running or isn’t on `PATH` for Waybar — start `buschain-control --hidden` and restart Waybar if needed.

---

## Docs

| Doc | Audience |
|-----|----------|
| [`docs/TECHNICAL.md`](docs/TECHNICAL.md) | Features detail, binaries, Waybar deep dive, env reference, architecture |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Live graph / engine contract |
| [`docs/HANDOVER-QUICKSHELL.md`](docs/HANDOVER-QUICKSHELL.md) | Quickshell rice contract (IPC schema, stubs) |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | Planned work |

---

## License

MIT — see [`LICENSE`](LICENSE).
