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
| Graph shim | `app/src/audio/graph.rs` | Session orchestration (apply/rewire/levels); node create/route via engine |
| PluginHost | `app/src/audio/plugin/` | Format backends behind traits |

**Process model:** one `buschain-control` process owns the graph
(in-process worker + coalesce). On startup it binds
`$XDG_RUNTIME_DIR/buschain-control/daemon.sock` and forwards ctl/waybar/GTK mixer
commands into the same worker. Close hides to tray; Quit tears the graph down.
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
Master HW (DeviceCaps)
        │
        ▼
 GraphClock  ← session.performance (rate + quantum + soft)
        │
        ├── buschain_track_* / buschain_master / buschain_post_* / buschain_hold / buschain_fx_*
        │
External 48 kHz mic ──► buschain_rs_* (rate bridge) ──► track bus @ GraphClock
```

- Config → **Apply audio settings** → `Command::BindMasterClock` → engine `Intent::BindMasterClock` then route rewire.
- Mismatched external endpoints get an owned `buschain_rs_*` null-sink at GraphClock; only that hop converts.

## PipeWire mixer graph

```
Apps ──assign──► Track null sinks ──► [LADSPA FX…] ──► Master ──► HW sink
Inputs ─assign─► (optional rate-bridge) ──► Track bus
```

- Master UI column is always leftmost (`Session::tracks_ui_order`).
- No hardcoded application tracks.

## Plugin backends

- **LADSPA / LV2 / CLAP** — always compiled; scan + (LADSPA) insert into graph.
- **VST3** — optional `vst3-carla` feature only; Carla used for VST3 discovery/load path exclusively; swappable via `PluginBackend` trait. Default build has zero Carla linkage.

## Built-in plugins (`plugins/`)

`buschain_denoiser`, `buschain_gate`, `buschain_eq`, `buschain_compressor`, `buschain_limiter`, `buschain_softclip`, `buschain_pitch`
