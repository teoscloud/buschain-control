# BusChain Control — Architecture

## Layers

| Layer | Path | Role |
|-------|------|------|
| Design system | `app/src/design/` | Tokens + Button/Toggle/Slider/Knob/Meter/Fader/Chip/TabBar/Panel |
| UI screens | `app/src/ui/` | Mixer, Playback, Recording, Devices, Config — no raw colors |
| Session | `app/src/session/` + `store.rs` | Named sessions, soft HW rebind, JSON under `sessions/` |
| IPC | `app/src/ipc.rs`, `daemon.rs` | Unix socket JSON embedded in the tray UI; `ctl` in `control/` |
| Live contract | `app/src/audio/live.rs` | `LiveChange` severity ladder → `AppState::commit` |
| Worker | `app/src/audio/worker.rs` | All audio I/O off UI thread; syncs `GraphClock` then calls graph/engine |
| **Engine** | `engine/` (`buschain-engine`) | **Only** place for PipeWire/Pulse capabilities — intents, clock, links, rate-bridges |
| **DSP host** | `engine/src/host/` | In-process insert rack (`PwFxNode` + LADSPA `process()`); PipeWire is mixer I/O |
| Graph shim | `app/src/audio/graph.rs` | Session orchestration (apply/rewire/levels); node create/route via engine |
| PluginHost | `app/src/audio/plugin/` | Format backends behind traits (UI/catalog); DSP runs in engine host |

**Process model:** one `buschain-control` process owns the graph
(in-process worker + coalesce). On startup it binds
`$XDG_RUNTIME_DIR/buschain-control/daemon.sock` and forwards ctl/waybar/egui mixer
commands into the same worker. Close hides to tray; Quit tears the graph down
**and must restore desktop audio** (HW default + unmute + destroy linger nodes) —
see TECHNICAL.md pin *quit leaves system audio broken*. Hide-to-tray must not.
Hyprland: `exec-once = buschain-control --hidden`. Do **not** run a separate
`buschain-daemon` unit — that forces a laggy thin-client hop.
`--daemon-client` remains a debug escape hatch only.

## Hard rule — no naked APIs in app

Application code must **not** add new `pactl` / `pw-link` / `pw-cli` / `pipewire` spawn sites.

1. Add or extend a `buschain_engine::contract::Intent` (or `AudioBackend` method).
2. Implement it in `engine/src/backend/`.
3. Call it from the worker / `engine_handle` / graph shim.

See [`engine/README.md`](../engine/README.md).

## Clock domains

```
Settings → Audio (engine catalog)
        │
        ▼
 GraphClock  ← session.performance (rate + quantum + soft; up to 384 kHz / DXD)
        │
        ├── buschain_track_* / buschain_master / buschain_post_* / buschain_glc_* / buschain_hold / buschain_fx_*
        │
        └── [buschain_rs_out_* if GraphClock ≠ HW rate] ──► Master HW sink
                                                              ▲
Master HW (DeviceCaps / device_clocks) ── force-rate only ────┘

External mic(s) ──shared──► [buschain_rs_* if rate ≠ GraphClock] ──► track bus(es)
     │
     └── desktop capture (Zoom/Discord) keeps its own links — never exclusive-stolen
```

- Settings → **Apply engine clock** → `Command::BindMasterClock` → `bind_graph_clock_profile` (no PW force-rate) then ClockBind rewire.
- Master HW → **Apply Master HW clock** → `BindDeviceClock(bind_buschain)` → force-rate HW only; GraphClock unchanged; egress rebuilds.
- Mismatched mics get inbound `buschain_rs_*`; mismatched Master→HW gets egress `buschain_rs_out_*`.
- Track capture is an **input rack** (`Track.inputs`): multiple HW sources per track, same source on many tracks; Desired `bus_inputs` reconciles shared hops via one-shot `Intent::SyncCapture` (Route never N× rebuilds capture).
- Apps rack pins use Desired `bus_playback` + one-shot `Intent::SyncPlayback` (never ApplyLevels / Route).
- **Master fan-in GLC** (`pipeline::glc`): nested Track→Track→Master paths are delay-compensated on the Master edge only (`buschain_glc_*` when δ>0); Track→Track stays undelayed. Egress cycles rejected. **Direct Out** (default on) opts stems out of GLC for low latency; Master Direct disables graph sync globally (host peer PDC).

## PipeWire mixer graph

Control plane: `PipewireNativeBackend` (`engine/src/backend/native/`) owns a dedicated
libpipewire MainLoop + registry cache for links / null-sinks / node lookup / levels
(SPA Props) / default sink (Metadata). Pulse remains only for stream-move
(`PulseCompat`) and emergency fallbacks.

**Reconnect:** the control plane is reconnectable (`mark_dead` / `reconnect_plane`).
After `systemctl --user restart pipewire pipewire-pulse wireplumber`, the supervisor
detects a hollow graph (`buschain_master` missing while Pulse is up, or plane dead),
reconnects the MainLoop, tears in-process FX hosts, cold-arms via
`Intent::ReconnectPipeWire`, then reasserts preferred default + reclaim — without
quitting the UI. Explicit Teardown suppresses auto-reconnect until the next Apply.

```
Apps ──assign──► Track null sinks ──► buschain_fx_* (in-process host) ──► post ──┬──► [glc δ?] ──► Master ──► HW
                                                                                 └──► other tracks (no GLC)
HW mics ─shared rack─► (optional rate-bridge) ──► Track bus(es)   [coexists with system default]
```

- Master UI column is always leftmost (`Session::tracks_ui_order`).
- No hardcoded application tracks.
- Insert FX is **not** an OS helper: `EnsureFxChain` publishes a rack into `PwFxNode`;
  knobs/power use a wait-free `ControlQueue` (no Props topology).

## Plugin backends

- **LADSPA** — v1 process path inside the host (`plugins/*/build/*.so`).
- **LV2 / CLAP** — scan in app; instantiate+process on the host is follow-up.
- **VST3** — discovery always on (Carla scanner); native host path always compiled (process activation still incomplete — see ROADMAP).

## Built-in plugins (`plugins/`)

`buschain_denoiser`, `buschain_gate`, `buschain_eq`, `buschain_compressor`, `buschain_limiter`, `buschain_softclip`, `buschain_pitch`
