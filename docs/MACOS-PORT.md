# macOS (Apple Silicon) portability brief

> Investigation only — **not a roadmap commitment** and **not an implementation plan**.
> BusChain Control remains a **Linux PipeWire** product. See [`TECHNICAL.md`](TECHNICAL.md)
> for supported Linux architectures (`x86_64-linux`, `aarch64-linux`).

## Verdict

**Limited shapes are possible; a faithful drop-in port is not.**

BusChain’s product identity is owning the desktop audio graph: virtual sinks/sources,
app reclaim, sealed wet FX spine, WirePlumber seal helpers, restore HW on quit. That
stack is inseparable from **PipeWire + pipewire-pulse + WirePlumber**. Core Audio has
no equivalent first-class “expose a sink other apps pick and route through your mixer.”

Apple Silicon CPU work already in-tree (aarch64 denormals, Linux `Contents/aarch64-linux`
plugin filtering) does **not** make `aarch64-darwin` buildable or runnable as Control.

| Port shape | Idea | Feasibility |
|------------|------|-------------|
| **B — Remote UI** | macOS egui / `buschain-ctl` → Linux PipeWire daemon | Highest near-term |
| **C — Scoped local host** | CoreAudio/cpal callback + insert rack/UI; no system app→bus routing | Medium; different product |
| **A — Full native system mixer** | Replace PW with aggregates / virtual devices / Audio Server Plug-In | Lowest; multi-year rewrite |

**Rank:** B ≫ C ≫ A.

Honest middle ground: **B** for continuity with a Linux box; **C** as a separate
“BusChain Host” if local DSP/UI matter; **A** is a new product, not a compile flag.

---

## Hard blockers (faithful Control)

| Blocker | Why |
|---------|-----|
| **PipeWire graph ownership** | Null sinks, FX filters, posts, rate bridges, Master→HW, reclaim defaults — entire engine (`engine/src/backend/native/`, `engine/src/host/node.rs`). |
| **Virtual app sinks / capture sources** | Tracks are Pulse-visible nodes other apps select. Core Audio aggregate / Multi-Output / third-party virtual drivers are a different architecture (often driver or Audio Server Plug-In). |
| **WirePlumber session policy** | Seal helpers via `session.ignore` ([`pipewire/wireplumber/.../51-buschain-seal-helpers.conf`](../pipewire/wireplumber/wireplumber.conf.d/51-buschain-seal-helpers.conf)). |
| **`pactl` / `pw-cli` / `pw-link`** | Still in the control plane for mute/volume/defaults, emergency destroy, probes (`engine/src/backend/cli.rs`, `null_sink.rs`, `fx_chain.rs`, `playback.rs`). |
| **`PwFxNode` RT path** | Insert DSP runs inside a PipeWire filter `process` callback ([`engine/src/host/node.rs`](../engine/src/host/node.rs)). Needs a new RT host (HAL IOProc / AVAudioEngine / custom graph). |
| **Wayland shell rice** | GTK layer-shell mixer, scroll strip, Waybar, Quickshell handover, Hyprland float — Linux desktop-shell features. |
| **Nix flake** | [`flake.nix`](../flake.nix) systems are `x86_64-linux` and `aarch64-linux` only; buildInputs assume pipewire, wayland, ALSA, gtk-layer-shell. |
| **ALSA device caps** | HW rates from `/proc/asound/...` ([`engine/src/clock/mod.rs`](../engine/src/clock/mod.rs)). |

These are not “swap a backend trait” items; they **are** the product.

---

## Soft blockers / workarounds

| Area | Darwin outlook |
|------|----------------|
| **egui / eframe** | Soft. Drop Wayland/X11 feature bias; remove `WINIT_UNIX_BACKEND=wayland` ([`app/src/main.rs`](../app/src/main.rs)). Cocoa backend exists upstream. |
| **VST3** | Soft→medium. Host is Linux Contents/ELF-centric ([`engine/src/host/arch.rs`](../engine/src/host/arch.rs), [`vst3.rs`](../engine/src/host/vst3.rs): `Contents/*-linux`, `.so`). Darwin needs `Contents/MacOS` / Mach-O (often universal). aarch64 FPCR denormals already help Apple Silicon DSP **if** hosted. |
| **CLAP** | Soft. Cross-platform format; scan/load must accept Darwin bundles/dylibs, not only `.so`. |
| **AU** | Hard for *this* codebase — **not implemented**. New host backend + `.component` packaging. |
| **LADSPA / LV2** | Soft technically; weak macOS ecosystem. Loaders assume `.so`. Vendored lilv `c_char` fix helps unsigned-char ARM ABIs. |
| **GTK mixer / layer-shell** | Soft if you keep egui popup; hard if you want layer-shell UX (`gtk-layer-shell` is Wayland-only). |
| **Tray (`ksni`)** | Soft/medium. StatusNotifierItem over D-Bus — not macOS. Need `tray-icon` / NSStatusItem / similar ([`app/src/tray.rs`](../app/src/tray.rs)). |
| **IPC Unix sockets** | Soft. Paths require `$XDG_RUNTIME_DIR` today ([`app/src/ipc.rs`](../app/src/ipc.rs)); on macOS use `dirs` runtime or `~/Library/...`. `SO_PEERCRED` is Linux-only (already no-ops off Linux). Remote UI (B) needs **network** IPC, which does not exist yet. |
| **MIDI** | Soft. Enumeration is PipeWire nodes (+ optional ALSA). Darwin → CoreMIDI. |
| **Hyprland float / VST3 surface** | Soft. `hyprctl` is opt-in Linux WM glue; editors can be normal Cocoa windows. |
| **Sandbox SHM helpers** | Soft with path/cred fixes (`buschain-plugin-dsp` / surface refuse unset `XDG_RUNTIME_DIR`). |

---

## Subsystem map

| Subsystem | Classification |
|-----------|----------------|
| Engine graph (null sinks, links, defaults, reclaim, reconnect) | **Linux-only** — rewrite for Core Audio aggregates / virtual devices |
| Filter-chain / sealed FX spine (`monitor→fx→post→Master`) | **Needs rewrite** — topology idea portable; PW plumbing is not |
| Plugin host rack (Rack, ControlQueue, process) | **Portable DSP core**; load/scan/RT glue **needs rewrite** |
| `PwFxNode` / native PW plane | **Linux-only** |
| Pulse compat / stream move | **Linux-only** |
| Full egui UI | **Portable** with winit backend cleanup |
| Tray / GTK / Waybar / QS / layer-shell | **Linux-only** (tray: replace; rest: drop or remote) |
| Packaging / Nix | **Linux-only** today |
| Sessions / themes | **Portable** (`dirs::config_dir()` → `~/Library/Application Support/...`) |
| MIDI learn / maps | **Portable logic**; I/O **needs rewrite** (CoreMIDI) |
| Clock / DeviceCaps | **Needs rewrite** (no `/proc/asound`) |

---

## What already helps (and what does not)

**Helps Apple Silicon *Linux*, or a future Darwin DSP host:**

- aarch64 FPCR flush-to-zero — [`engine/src/host/denormal.rs`](../engine/src/host/denormal.rs)
- Host-arch plugin filtering — [`engine/src/host/arch.rs`](../engine/src/host/arch.rs) (today: ELF + `Contents/*-linux` only)
- Session JSON / themes via `dirs`
- IPC peer-cred already stubs off Linux
- Insert rack / ControlQueue / catalog design

**Does not mean darwin is supported:**

- Flake + CI cover `x86_64-linux` and `aarch64-linux` only
- No Core Audio, AU, Cocoa tray, or Mach-O VST3 paths in-tree
- “Compile for `aarch64-darwin`” is **not** the next milestone

---

## Port shapes in more detail

### B — Remote UI (highest feasibility)

macOS runs the control surface (or a thin `buschain-ctl`); audio graph stays on a
Linux PipeWire machine (LAN / VPN / SSH tunnel).

- **Reuse:** session concepts, mixer UX, most of the product value
- **Required work:** transport beyond local Unix sockets (TLS or SSH-forwarded socket);
  auth model (today: same-UID + `SO_PEERCRED` on Linux)
- **Not solved:** “BusChain as the Mac system mixer”

### C — Scoped local host (medium)

Local device I/O + insert rack + egui; no virtual sinks for Chrome/Discord/etc.

- **Reuse:** rack, plugin catalog ideas, UI chrome, denormals on aarch64
- **Required work:** CoreAudio (or cpal) callback instead of `PwFxNode`; Darwin VST3/CLAP
  discovery; tray/IPC path fixes; drop Wayland rice
- **Product honesty:** this is closer to a plugin host than Control

### A — Full native system mixer (lowest)

Replace PipeWire with Core Audio aggregates, virtual devices, and app routing policy.

- Often needs an **Audio Server Plug-In**, kernel extension–class virtual device, or
  third-party tools (BlackHole-class) users must install
- Reinvent WirePlumber-equivalent session policy
- Treat as a **greenfield** product sharing UI/DSP ideas, not a port

---

## Concrete Linux-assumption cites

**Engine / graph**

- [`engine/src/host/node.rs`](../engine/src/host/node.rs) — PipeWire filter RT node
- [`engine/src/backend/native/`](../engine/src/backend/native/) — libpipewire control plane
- [`engine/src/backend/cli.rs`](../engine/src/backend/cli.rs), [`null_sink.rs`](../engine/src/backend/null_sink.rs), [`pulse_compat.rs`](../engine/src/backend/pulse_compat.rs)
- [`engine/src/clock/mod.rs`](../engine/src/clock/mod.rs) — `/proc/asound`
- [`pipewire/wireplumber/wireplumber.conf.d/51-buschain-seal-helpers.conf`](../pipewire/wireplumber/wireplumber.conf.d/51-buschain-seal-helpers.conf)

**App / desktop**

- [`app/src/main.rs`](../app/src/main.rs) — Wayland winit default
- [`app/src/tray.rs`](../app/src/tray.rs) — `ksni` / D-Bus
- [`packaging/nix/gtk-mixer.nix`](../packaging/nix/gtk-mixer.nix) — gtk-layer-shell
- [`app/src/hyprland_float.rs`](../app/src/hyprland_float.rs) — `hyprctl`
- [`packaging/waybar/`](../packaging/waybar/), [`docs/HANDOVER-QUICKSHELL.md`](HANDOVER-QUICKSHELL.md)

**IPC / runtime**

- [`app/src/ipc.rs`](../app/src/ipc.rs) — `XDG_RUNTIME_DIR`, Linux `SO_PEERCRED`
- Plugin DSP/surface helpers refuse unset `XDG_RUNTIME_DIR`

**Plugins / arch (Linux aarch64, not Darwin)**

- [`engine/src/host/arch.rs`](../engine/src/host/arch.rs), [`vst3.rs`](../engine/src/host/vst3.rs)
- [`flake.nix`](../flake.nix), [`.github/workflows/nix.yml`](../.github/workflows/nix.yml)

**MIDI**

- [`engine/src/midi/enumerate.rs`](../engine/src/midi/enumerate.rs) — PipeWire / `pw-cli`
- Optional ALSA seq (`midi-alsa`)

---

## Non-goals

- Shipping `aarch64-darwin` flake outputs as if Control were portable
- Loading Linux x86_64 VST3/CLAP under emulation inside the realtime path (FEX/box64)
- Claiming commercial macOS plugin parity without AU / Darwin VST3 host work
- Treating this brief as scheduled engineering work

---

## Bottom line

The valuable portable assets are **session/UI ideas + insert rack design**. The
**system mixer** is PipeWire. For macOS users of this codebase, **remote UI (B)** or a
**scoped local host (C)** are the only credible near-term shapes; **full native (A)**
is a different product.
