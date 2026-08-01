# BusChain Control — Technical reference

> Install and everyday usage: root [`README.md`](../README.md).

Linux **PipeWire system mixer**: dynamic tracks, insert FX, named sessions, and
Master HW volume for the desktop shell. One tray-resident process owns the
audio graph; close **hides**, Quit tears it down.

Standalone Nix flake — build and run from this repository.

**Dig deeper:** [`ARCHITECTURE.md`](ARCHITECTURE.md) ·
[`packaging/waybar/`](../packaging/waybar/) ·
[`HANDOVER-QUICKSHELL.md`](HANDOVER-QUICKSHELL.md) ·
[`.cursor/rules/live-graph.mdc`](../.cursor/rules/live-graph.mdc)

---

## Features

### Mixer & graph

- Dynamic **tracks / buses** with mute, solo, listen, and gain
- **Input rack** per track (multi HW capture, shared with desktop / other tracks)
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

- Tray icon (left-click popup · Open full app / Hide / Quit)
- Waybar pill optional; Master HW scroll via Quickshell strip or opt-in GTK strip
- Mixer popup: Quickshell (opt-in) → GTK layer-shell → egui `--popup` last

### Sessions

- Named library under `~/.config/buschain-control/sessions/`
- Soft-bind on load: missing HW/mics rebound by description or cleared; mix + FX kept
- Mixer favorites: `mixer-pins.json`

---

## UI surfaces

| Surface | Role |
|---------|------|
| **Full egui window** | Mixer, Playback, Recording, Output/Input devices, MIDI, Settings (tray menu) |
| **Quickshell mixer + strip** | Preferred for Quant / Hyprland — see [`HANDOVER-QUICKSHELL.md`](HANDOVER-QUICKSHELL.md) |
| **GTK layer-shell panel** | General-desktop tray/bar popup (QS-like tabs) when `buschain-mixer-gtk` is available |
| **egui `--popup`** | Last-resort mixer when QS/GTK are unavailable |

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
`vst3PluginRuntimeLibs` in [`flake.nix`](../flake.nix).

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

**One writer:** tray daemon `AdjustHwVolume` / `apply_master_hw_volume` (hard
cap 100%, ±5% grid). No bash `pactl` dual-path.

**Hover scroll is BusChain-owned** — a transparent GtkLayerShell strip over the
Waybar pill receives real wheel/touchpad delta and applies N notches. Stock
Waybar `on-scroll-*` cannot do 1:1 (SMOOTH magnitude is discarded after one
forkExec). Do **not** put `hw-vol up|down` on the Waybar module.

```bash
buschain-ctl status
buschain-ctl hw-vol get|set <pct>|up|down|mute toggle
buschain-waybar popup          # click → QS → GTK → egui
```

Scroll strip geometry: [`packaging/scroll-strip/README.md`](../packaging/scroll-strip/README.md).
GTK strip opt-in: `BUSCHAIN_CONTROL_SCROLL_STRIP=1`.

---

## Waybar + mixer popup

**Start tray → optional Waybar pill.** BusChain owns ctl + IPC. Tray / pill
click uses **QS → GTK → egui**. Snippets: [`packaging/waybar/`](../packaging/waybar/).

### Behavior

| Action | Behavior |
|--------|----------|
| Status pill | `buschain-ctl status` → Master HW % (interval + RTMIN+9) |
| Scroll | QS strip or opt-in GTK strip → `AdjustHwVolume` (±5%, cap 100%) |
| Click | Module / tray → mixer popup (QS → GTK → egui) |

Click path: `buschain-waybar popup` → `buschain-ctl popup` → QS → GTK → egui `--popup`.

### Checklist

1. Tray running (`cargo run -- --hidden`)
2. Paste `custom/buschain-control` from [`packaging/waybar/module.jsonc`](../packaging/waybar/module.jsonc) (**no** `on-scroll-*`)
3. Optional: `BUSCHAIN_CONTROL_SCROLL_STRIP=1` + align `BUSCHAIN_CONTROL_SCROLL_*` if using legacy GTK strip
4. Pill CSS optional; restart Waybar after config/CSS edits

### Module (waybar `config`)

```jsonc
"modules-left": [
  "hyprland/window",
  "custom/buschain-control"
],

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

Same object: [`packaging/waybar/module.jsonc`](../packaging/waybar/module.jsonc).
**Do not add `on-scroll-up` / `on-scroll-down`** — that fights the strip and still skips coalesced notches.

#### Packaged / on PATH

```bash
nix profile install github:teoscloud/buschain-control
```

Use the short names above. Restart Waybar if it was started before the profile was on PATH.

#### Git checkout (no install)

```jsonc
"custom/buschain-control": {
  "format": "{}",
  "return-type": "json",
  "exec": "<checkout>/target/debug/buschain-ctl status",
  "interval": 3,
  "signal": 9,
  "exec-on-event": false,
  "on-click": "<checkout>/packaging/waybar/buschain-waybar popup",
  "tooltip": true
}
```

Run from `nix develop` + `cargo run -- --hidden` so ctl, mixer, and scroll strip exist.

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

Same file: [`packaging/waybar/style.css`](../packaging/waybar/style.css).

### Autostart (Hyprland)

```conf
exec-once = buschain-control --hidden
exec-once = waybar
```

The pill shows offline until the tray owns the IPC socket.

### Home Manager (optional — bins only)

Not required. Use only if you want the tray autostarted and bins on PATH; you
still paste the Waybar module yourself.

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
| 1 | Quickshell mixer | `BUSCHAIN_CONTROL_QS_MIXER=1`, toggle script, or `qs` on PATH |
| 2 | GTK layer-shell | `buschain-mixer-gtk` available (opt out: `USE_GTK_MIXER=0`) |
| 3 | egui `--popup` | last resort |

Opt out of GTK popup: `BUSCHAIN_CONTROL_USE_GTK_MIXER=0`.  
Opt in GTK scroll strip: `BUSCHAIN_CONTROL_SCROLL_STRIP=1`.  
Skip GTK strip when QS owns it: `BUSCHAIN_CONTROL_QS_STRIP=1`.  
Override mixer binary: `BUSCHAIN_CONTROL_MIXER=/path/to/buschain-mixer-gtk`.  
Quant handover: [`HANDOVER-QUICKSHELL.md`](HANDOVER-QUICKSHELL.md) · stubs in [`packaging/quickshell/`](../packaging/quickshell/).

### Troubleshooting

| Symptom | Fix |
|---------|-----|
| `vol —` / class `offline` | Start tray; check socket exists |
| Click does nothing | `on-click` → `buschain-waybar popup` and tray running |
| Click OK, no panel | Tray running; GTK needs `gi` + layer-shell (`nix develop`); else egui `--popup` |
| Scroll works, status stale | `signal: 9`; tray sends RTMIN+9; restart tray + Waybar |
| Scroll skips / need to spam wheel | Remove Waybar `on-scroll-*`. Prefer QS strip; legacy GTK needs `SCROLL_STRIP=1` + align geometry. |
| No scroll / strip missing | Expected unless `SCROLL_STRIP=1` or QS strip; set env and rebuild tray |
| ctl / helper not found | Build ctl (`cargo build -p buschain-tools --bin buschain-ctl`); absolute path for bare Waybar PATH |
| VST3 editor tiled (Hyprland) | Ensure `hyprctl` works; add the optional `windowrulev2` above |
| VST3 missing `.so` | Expand `vst3PluginRuntimeLibs` in `flake.nix` |
| Desktop silent after Quit / kill | `buschain-ctl recover-audio` · see [PIN](#pin--quit-leaves-system-audio-broken). Blunt: `systemctl --user restart wireplumber` |

```bash
buschain-waybar status
buschain-ctl status
command -v buschain-mixer-gtk
command -v buschain-plugin-surface
```

---

## Pins / follow-ups

### PIN — quit / graph edits leave system audio broken

**Status:** hard invariants + graceful restore landed (2026-07); residual = crash/`kill -9` and WP races.

**Symptom:** After closing, killing, **+Track**, or assigning a system mic as track input, PipeWire can stay hollow — default on dead `buschain_*`, Master HW muted, apps on linger null sinks, or Master→HW cold-disarmed. Desktop capture can also die when exclusive mic hops steal the default source.

**Field note (2026-07-31):** A NixOS rebuild that **restarted `wireplumber.service`** cleared hollow desktop audio. Blunt recovery: `systemctl --user restart wireplumber`.

**Mitigations in tree:**

- Quit / Teardown → `restore_system_audio` + linger destroy; **keep sticky `preferred_default_sink` as buschain_*** across quit (live PW default still restored to HW). Reopen reasserts preferred + reclaim burst so apps rewire without clicking System default.
- **Dual-lane preferred ownership:** Supervisor Full Apply publishes an authoritative shared-session generation after `ensure_buschain_preferred_default`. Interactive must not overwrite a newer gen (ApplyMidiConfig is MIDI-only merge). Supervisor only adopts shared when `shared.gen >= local.gen`. Reopen is not done until Pulse `Default Sink` matches sticky `buschain_*` and reclaim has run.
- **Verify-after-set:** native metadata set-default is provisional; success requires Pulse (`pactl info`) agreement, else fall through to pactl/wpctl. Apply + ~10s reclaim burst retry both `set_default_sink_if_needed` and `sync_playback`.
- Surgical commits (`EnsureTrack` / `Route` / `VirtualInput` / `FxRewire`) **never** escalate to Full Apply when the snapshot looks cold — only `Reconcile` may ArmSession
- Preferred `buschain_*` default: while the session owns the graph, keep asserting preferred (e.g. Linux track virtual out) even if Master→HW is healing — do **not** force Scarlett mid-session. HW restore + stream move-off is Quit/Teardown only. Playback reclaim pulls unpinned / Hold / HW apps onto preferred when that sink exists. soft_bind does **not** clear missing `buschain_*` preferred while the graph is coming up.
- Capture is **shared** (`exclusive: false`); Desired `bus_inputs` reconcile + sink-side unlink
- Wet Master hold mid-build fail-opens dry Master→HW after ~2s
- Session load prune uses the same `teardown_track_bus` as UI delete (`destroy_node` + virtual-input teardown)
- Input rack: `Track.inputs: Vec<TrackInput>` (multi-source, mute per row); same mic on many tracks OK
- `buschain-ctl recover-audio` — pactl restore without tray
- **Route is one-shot** (`Intent::SyncCapture` + egress relink) — not N× per-track purge/rebuild (graph-apply lag, not buffer delay)
- **PruneTrack silence-first** — gate + disarm egress + unlink capture before FX/vin destroy
- Desired `buschain_rs_*` kept during capture purge; sink-side dual-path prune when bridging
- `relink_routes` never ArmSession / ForceRespawn (soft-arm latch only)
- **Mic Pulse flap sealed:** `link_is_live` / `ensure_link` treat live `module-loopback` as already-ok (no unload→reload silence); wait for `buschain_rs_*` ports after create
- **Apps rack:** PlaceApp / `Intent::SyncPlayback` only (no ApplyLevels HOL); **move/retarget** onto the track bus (never clone/loopback). **Discovery is native-first**: `list_sink_inputs` reads `Stream/Output/Audio` nodes + app props from the native registry (`list_playback_streams`) so streams on `Audio/Sink/Internal` buses stay visible even when pipewire-pulse hides them; pactl only enriches volume/mute. Observer Class C is generation-driven (registry bump → refresh within ~300ms; 5s max interval; 1.5s cadence only in Pulse-fallback mode). Engine reclaim (`enforce_desired_playback`) uses the same native list and the same `app_key` cascade (`binary_is_generic` shared semantics). Streams parked on Hold stay user-visible for reclaim. PlaceApp goes through `pulse_compat::move_sink_input` (native `target.object` retarget first — required for `Audio/Sink/Internal` non-VO tracks); after a successful place on an FX track, one-shot wet `arm_track_egress`.
- **Null-sink exposure:** Master / VO use `media.class = Audio/Sink` (Pulse-visible). Helpers (Hold, Post, rate-bridge, vin feed, non-VO tracks) use standard **`Audio/Sink/Internal`** — ports work (unlike custom `BusChain/Internal`, which is forbidden) and stay out of pavucontrol. Also stamp `buschain.pulse.export` + optional WirePlumber rules (`pipewire/wireplumber/…` / `scripts/install-wireplumber-rules.sh`). PlaceApp/reclaim prefer native `target.node` retarget. Pulse `module-loopback` onto Hold/helpers is forbidden; Apply sweeps leftover BusChain loopbacks.
- **Meters:** wet strips use in-process FX host peaks; dry strips use a native `{bus}.monitor → buschain_mtr_*` peak tap. Pulse `meter-*` streams stay opt-in only (`BUSCHAIN_PULSE_METERS=1`).
- **VO toggle** flips `pulse_export` → recreate null-sink as `Audio/Sink` ↔ `Audio/Sink/Internal` (stream remount) without ArmSession / ForceRespawn. RT path hop count unchanged.
- **Non-linger graph:** native null sinks + links set `object.linger = false` so process exit/crash drops BusChain nodes. Create proxies are **retained for the process lifetime** (dropping them with linger=false removed VO/Master from the graph). Quit still restores HW + module unload + `pw-cli` destroy; `recover-audio` destroys Pulse-invisible helpers; startup only sweeps broken `BusChain/*` leftovers (never a full wipe — that raced Apply).
- **Wet egress heal:** never prune `post.monitor→dest` while an FX host/gen is live; idle/prune re-arms missing post→Master/HW (half-up wet left meters alive and speakers silent).
- **Sealed track chain:** when FX is Desired/host-up, `arm_track_egress` must soft-cutover (keep `bus→fx`) — never fall through to dry unlink that strips the FX feed (that sent apps/mics dry into Master and left track inserts silent).

- **Apps require a Pulse-visible track (`virtual_output` / `Audio/Sink`).** Chromium and Electron (Equibop, Vesktop, Cider, …) talk through `pipewire-pulse` and **hang forever** when retargeted onto `Audio/Sink/Internal` — the client never finishes the move and cannot switch devices until the pin is removed. Assigning an app auto-enables System virtual output and recreates the bus as `Audio/Sink` before place. Native-only clients (e.g. Brave) can survive Internal, which is why some apps "worked" and others froze.
- **Electron identity:** `application.name = "Chromium"` is treated as generic; keys prefer real `application.process.binary` / `application.id` so Equibop ≠ Brave. Retarget is a no-op when the stream is already linked (Chromium pauses on every rewrite).
- **`target.object` must be the node NAME (or `object.serial`), never the node id.** WirePlumber resolves `target.object` by matching `node.name`/`object.serial` and it takes precedence over the legacy `target.node` (which *is* the node id). Writing the node id into `target.object` made every target unresolvable, so WirePlumber silently fell back to the default sink — apps appeared to ignore their track assignment and stayed on the system default. `set_stream_target_node` now writes `target.object` = name (`Spa:String`) plus `target.node` = id (`Spa:Id`).
- **Filter nodes must not declare a `media.class`.** `PwFxNode` (FX hosts and `buschain_mtr_*` taps) sets only `media.type/category/role` + `node.virtual`. Stamping `media.class = Audio/Duplex` made pipewire-pulse register every filter as a device that never reports sample/map/volume, logging `sink not ready` in a hot loop (700k+ lines/day) and wedging the whole Pulse layer: `pactl list sink-inputs` returned empty, `parec` produced zero bytes, and new Pulse clients were never routed. That single property was the real cause of "no apps show" / "apps not placed on tracks".
- **Non-finite guard:** the FX host scrubs NaN/Inf on both sides of the rack. Non-finite state in an IIR/delay never decays, so one bad block would silence a bus permanently.

- **Output to… may omit Master.** Empty destinations mean hold-only (intermediate bus / track→track without a master send). New tracks still default to Master; Listen (AFL) still forces Master. Desired `bus_egress` empty is authoritative — it must not fall through to a Master default.
- **System virtual input does not imply Master.** The vin-feed sink (`buschain_vinf_*`) is a remap-source only — its monitor must stay hold-only (plus Pulse `input.buschain_vin_*` capture). Idle heal used to default helper buses to Master (`vinf.monitor → buschain_master`), so toggling virtual input re-audibled DualMic on Master even with Output empty. Helpers never get a Master egress default; reconcile surgically unlinks feed→Master/track leaks only (never wipe-all — that stripped remap capture and silenced `buschain_vin_*`). `ensure_virtual_input` bounces the remap module if feed.monitor→input.vin hops are missing.

### Standard track wiring contract

Canonical per-bus node set and the only legal links. Anything else is pruned; nothing here may be stripped by any other path.

| Link | When | Owner |
|------|------|-------|
| `{bus}.monitor → buschain_hold` | **always** (keepalive; hard-protected in unlink) | arm/reconcile |
| `{bus}.monitor → buschain_fx_X` | FX Desired (kept through mid-build holds) | arm / ensure_fx_chain |
| `{bus}.monitor → buschain_mtr_X` | no live FX host (dry strip meter; hard-protected in unlink) | `host::dry_meter` only |
| `{bus}.monitor → dest` | dry, or soft-cutover **per dest** while that dest's wet hop is down | arm |
| `buschain_fx_X → buschain_post_X` | FX Desired | ensure_fx_chain |
| `{post}.monitor → dest` | wet | arm |

Cutover rules:

- **Per-dest exclusivity:** once `post.monitor→dest` is live, the dry `{bus}.monitor→dest` for *that* dest is dropped; dests still waiting keep dry (no all-or-nothing silence, no double audio).
- **Never prune dry before wet lands:** idle prune drops a dry `bus→dest` only when that dest's `post→dest` is live (`prune_parallel_fx_routes`).
- **Master mid-build hold keeps the FX feed** (fx + mtr allow-listed) — a hold-only allow-list is the historical sealed-chain bug.
- **`buschain_mtr_*` lifecycle:** created only by `host::dry_meter` when no host is live; torn down when a host comes up (including the ensure-race re-check), on track delete / orphan prune / Teardown / Quit, and when the UI hides (`sleep_visualization` — no RT peak-scanning while nothing renders).
- **Dry egress heal (`heal_dry_egress`):** `prune_parallel_fx_routes` only walks `fx_chains`, and a track with zero inserts never gets a chain spec, so nothing re-armed `bus→dest` for it after a sweep / WirePlumber restart / device recreate. The strip kept metering (mtr tap and hold are separate links) while being silent to Master. The heal runs each idle tick over every non-Master bus that is dry, unmuted, and missing a configured dest.

**Still open:**

- Thin-client `--daemon-client` quit leaves external graph alone
- Automated smoke: Quit → HW default unmuted + no orphan `buschain_track_*`; +Track / mic assign → Master→HW link count never zero; Add In / Delete track → take effect &lt; ~1s

**Manual recovery:**

```bash
buschain-ctl recover-audio
# or blunt:
systemctl --user restart wireplumber
```

### Smoke matrix (manual)

Interactive control-plane gate (`BUSCHAIN_CONTROL_LAT_TRACE=1`):

| Step | Expect |
|------|--------|
| Track mute LED | Silence &lt;20ms; one `SetTrackLevel`; no N× gate; `[lat] A gate_track_mute … OK` |
| In **M** / **×** | `[lat] B CaptureDelta …`; mic silent; no `RewireSessionRoutes` |
| Re-Add mic after × | Audible &lt;~200ms; `capture live …` (never empty SyncCapture); no ghost In |
| Idle 10s while muted | Stays muted (no open_bus_gain undo) |
| Apps Add during In edit | PlaceApp completes; mute still Class A priority |
| Start player after BusChain up | App appears in Apps Add ≤2s (native registry generation-driven; no Apply) |
| Pin Chromium to non-VO FX track | Stream on that Internal bus **and still listed in Apps** (native discovery); pitch/EQ audible; no dry `track.monitor→master` |
| App on Internal track after restart | Reclaim/pin still sees it (native `enforce_desired_playback`) |
| Dry track with mic/app, no inserts | Strip meter moves (native `buschain_mtr_*`; Pulse meters still off by default) |
| Hide → show mixer window | Dry-meter filters torn down while hidden (no RT cost); meters move again on show |
| Sealed wet path after PlaceApp | App → track → fx → post → Master; inserts hear the stream |
| Add first insert to a busy dry track | No silence gap and no double audio during soft-cutover (per-dest exclusivity) |

| Step | Expect |
|------|--------|
| Quit from tray / kill process | Default sink/source = HW, unmuted; no `buschain_*` nodes in `pw-cli ls Node` |
| pavucontrol Output Devices | HW + Master + VO tracks only (no Hold / Post / RateBridge) |
| +Track with live Master | Master→HW stays linked; desktop playback continues |
| Add Scarlett mic to track while Discord captures | Discord keeps the mic; track meters show signal |
| Same mic on two tracks | Both tracks get signal; no exclusive steal |
| Load session with fewer tracks | Mixer strip count matches JSON; `pactl list short sinks` has no orphan `buschain_track_*` |
| Clear all In rows | Capture hops unlinked; desktop mic unchanged |
| Add mic to track | Meters/signal within ~1s; no silence flap / repeated Pulse WARN |
| Delete track feeding Master | Master stops that feed immediately (silence-first prune) |
| Assign app to track (Apps rack) | Stream on target bus within ~1s; no ApplyLevels HOL |
| Unpin app from track | Leaves track quickly (preferred default / Master) |
| pavucontrol Output | HW + Master + VO tracks only (no Post/Hold/RS/non-VO) |
| pavucontrol Playback/Recording | No `loopback-*` / `buschain-control-meters`; event/"System Sounds" not reclaimed onto BusChain |
| VO toggle | No ArmSession; streams remount; FX edit latency unchanged |

### Structural edit latency budget

| Edit | Severity | Rule |
|------|----------|------|
| Fader / mute / Props | Class A | Never Ensure/Arm; SPA Props |
| PlaceApp | Class B | SyncPlayback + native retarget; no ApplyLevels HOL |
| Route / Capture | Surgical | Idempotent links; no ArmSession |
| FX add/remove/reorder | FxRewire | Async gen-swap; UI stays live |
| VO / vin toggle | Surgical | media.class flip + remount; no Arm |
| Clock / Full Apply | Heavy | Explicit Apply / Reconcile only |

---

## Environment reference

| Variable | Role |
|----------|------|
| `WINIT_UNIX_BACKEND` | Host window system; default forced to `wayland` if unset |
| `BUSCHAIN_VST3_SURFACE` | `0` / `false` / `off` disables surface promote |
| `BUSCHAIN_SANDBOX_ALL` | Sandbox all non-trusted inserts |
| `BUSCHAIN_SANDBOX_PLUGINS` | CSV of plugin keys to sandbox |
| `BUSCHAIN_CONTROL_SKIP_BOOTSTRAP` | Skip debug bootstrap on `cargo run` |
| `BUSCHAIN_CONTROL_USE_GTK_MIXER` | `0` disables GTK popup (default on when packaged) |
| `BUSCHAIN_CONTROL_MIXER` | Override GTK mixer binary |
| `BUSCHAIN_CONTROL_SCROLL_STRIP` | `1` enables legacy GTK Master HW scroll strip (default off) |
| `BUSCHAIN_CONTROL_SCROLL_STRIP_BIN` | Path to scroll-strip launcher |
| `BUSCHAIN_CONTROL_SCROLL_ANCHOR` | `left` / `right` (strip over pill) |
| `BUSCHAIN_CONTROL_SCROLL_MARGIN_TOP` / `_MARGIN_X` / `_WIDTH` / `_HEIGHT` | Strip geometry (px) |
| `BUSCHAIN_CONTROL_QS_MIXER` | Prefer Quickshell mixer (also auto if toggle script / `qs`) |
| `BUSCHAIN_CONTROL_QS_STRIP` | Skip GTK Master HW scroll strip (QS owns strip) |
| `BUSCHAIN_CONTROL_CTL` | Path to `buschain-ctl` |
| `BUSCHAIN_CONTROL_DAEMON` | Socket path override |
| `BUSCHAIN_CONTROL_USE_DAEMON` | Thin-client debug |
| `BUSCHAIN_CONTROL_LAT_TRACE` | `1` → Class A/B/C latency lines (`OK`/`SLOW`/`FAIL`) |
| `BUSCHAIN_ALLOW_PULSE_CAPTURE` | Escape hatch: allow Pulse loopback when native registry is up |
| `BUSCHAIN_PULSE_METERS` | `1` enables Pulse dry-bus meter streams (default off — host wet + native dry taps) |
| `VST3_PATH` / `CLAP_PATH` / `LV2_PATH` / `LADSPA_PATH` | Plugin scan roots |
| `HYPRLAND_INSTANCE_SIGNATURE` | Enables Hyprland float dispatch for editors |

---

## Architecture

One `buschain-control` process owns the graph via a **dual-thread control plane**:

| Lane | Thread | Owns |
|------|--------|------|
| **Interactive** | `buschain-interactive` | Mute/fader/Props, `SyncCaptureDelta`, PlaceApp (Class A/B) |
| **Supervisor** | `buschain-supervisor` | Route/Ensure/Prune/ArmSession/FxRewire, idle reconcile (Class C) |
| **Observer** | `buschain-observer` | Generation-driven `SinkInputs` (native registry bump; 1.5s Pulse fallback) + on-demand `refresh_snapshot` — never ENGINE mute path |

Sealed FX pillars stay unchanged: `pipeline/arm` disarm/arm, gen-swap ForceRespawn
(Supervisor/FxRewire only — in-process rack swap; the old A/B `__stg` staging null-sinks
are **retired**, remaining `__stg` destroys are leftover hygiene only), `speakers_armed`
/ ArmSession barrier, ControlQueue Props, never-cork app sinks, PruneTrack silence-first.
Capture hops verify native-live before `last_applied` updates (`Intent::SyncCaptureDelta`).

The engine insert rack (`PwFxNode` + `AudioProcessor` host) is the DSP path;
PipeWire is mixer I/O. Helpers (`plugin-surface` / `plugin-dsp`) run only when
promoted or sandboxed.

| Tree | Role |
|------|------|
| `engine/` | PipeWire intents, GraphClock, insert host (LADSPA/CLAP/VST3/LV2), MIDI, sandbox SHM |
| `app/` | Tray UI, session store, in-process worker, embedded IPC |
| `control/` | `buschain-ctl`, plugin helpers, legacy daemon |
| `plugins/` | BusChain LADSPA builtins |
| `packaging/` | Waybar / GTK mixer / scroll strip / desktop files |

Warm restart: if the live graph already matches the session, adopt (no FX ForceRespawn).

Details: [`ARCHITECTURE.md`](ARCHITECTURE.md), [`engine/README.md`](../engine/README.md).

---

## App tabs

Mixer · Playback · Recording · Output · Input · MIDI · Session · Settings

---

## License

MIT — see [`LICENSE`](../LICENSE).
