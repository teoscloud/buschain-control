# buschain-engine

First-class audio engine for BusChain Control. **App code must not call `pactl` / `pw-link` / `pw-cli` / `pipewire` directly** — every capability lives here as a typed contract. If a feature needs something new, extend this crate first.

## Layers

| Module | Role |
|--------|------|
| `contract` | Public intents (`EnsureBus`, `SetRoute`, `EnsureFxChain`, `ArmSession`, …) |
| `domain` | Typed node/link/clock/insert identities |
| `clock` | `GraphClock`, `PerformanceProfile`, device probe + resolve |
| `plan` | Desired state (buses, routes, bridges, **fx_chains**, **bus_egress**, `speakers_armed`) |
| `host` | **In-process DSP host** — `PwFxNode`, LADSPA/CLAP/VST3 rack, PDC, meters, SPSC `ControlQueue` |
| `pipeline` | Sealed insert wet-switch + **arm** (dwell, disarm/arm egress) |
| `backend` | `AudioBackend` — **native** registry plane (`native/`) for links/null-sinks/lookups; Pulse CLI for levels |
| `runtime` | `Engine` handle used by the worker |

## Clock domains

- **GraphClock** = Master HW rate + quantum after `BindMasterClock` (`PerformanceProfile`).
- **DeviceCaps** = true rates from PipeWire `EnumFormat` ∪ ALSA `/proc/asound/cardN/stream*` `Rates:`.
- Rate bridges are **inbound only** (external → BusChain bus); capture is **shared**
  (`exclusive: false`) so desktop apps keep the mic.
- Outbound Master→HW is never bridged (device adapter).
- Route / Add In: `Intent::SyncCapture` once + egress `relink_routes` (never ArmSession).

## Insert pipeline (sealed DAW contract)

**Plugins are black boxes. The mixer only declares the rack. The engine owns the wire.**

```
apps → buschain_track_* / buschain_master
         │
         ▼
   {bus}.monitor ──► buschain_fx_{bus}   (PwFxNode / in-process LADSPA rack)
                         │
                         ▼
                  buschain_post_{bus}   (meter tap)
                         │
                         ▼
                  Master / HW dest
```

| Intent | Effect | Forbidden |
|--------|--------|-----------|
| `PushFxControls` | SPSC `ControlQueue` → slot params / bypass fade | Respawn; Props topology; Engine mutex on hot path |
| `EnsureFxChain(Force)` | Build next `Rack` off-thread; atomic publish; link spine | Second FX process; A/B `__stg` helpers |
| `EnsureFxChain(Idempotent)` | No-op when fingerprint + host running + spine match | Silent dry when rack non-empty |
| `ArmSession` | Cold: spines → barrier → Master→HW. Warm: adopt healthy graph | App-side `pw-link` arming |
| `SyncCapture` | Desired `bus_inputs` → shared capture hops (one-shot Route) | N× per-track purge; ForceRespawn |
| `SyncPlayback` | Desired `bus_playback` → move sink-inputs (one-shot Apps) | ApplyLevels HOL; N× per-key list |
| `TeardownFxChain` | Destroy filter + drop rack for one bus | — |

Types: `InsertSlot` → `ChainSpec` → `WirePlan` → `ChainState::{Dry,Building,Wet,Failed}`.

Hard rules:

- No heap / mutex / syscall / logging in the PW filter `process` callback (drain queue + `Rack::process` only).
- Structural edits = generation swap; never `pipewire -c` filter-chain helpers.
- Click-free bypass via short crossfade (`host/fade.rs`).
- Keepalive `{bus}.monitor → buschain_hold` always preserved.
- v1 process path: **LADSPA** builtins. CLAP/VST3 share `AudioProcessor` later.

## GraphSupervisor (`Engine::reconcile`)

Continuous Desired↔live health. Idle worker ticks + after Hotplug sync session into
`DesiredState` then call `reconcile()` / `reconcile_light()`:

- Ensure declared buses + keepalive
- Open bus gains (never leave create-mute@0)
- Idempotent FX ensure for non-empty racks (host fingerprint)
- Master→HW only when `speakers_armed` (post when wet)
- Preferred default sink watchdog (debounced)

## Live Graph rules

- **Levels / mute** — never mute or recreate app-facing buses; silence outbound monitors / FX only.
- **FX power / knobs** — `PushFxControls` → host queue only.
- **FX add/remove/reorder** — `EnsureFxChain(Force)` once per track (gen-swap).
- **Rate bridges** — inbound external→BusChain only; mic hops shared (sink-side dual-path prune).
- **Custom clock** — device-capable rates only; Apply switches HW + GraphClock together.

## Native PipeWire control plane (Phase F)

`PipewireNativeBackend` (default) runs a dedicated `buschain-pw-ctrl` MainLoop thread with a live registry cache:

- **Hot path (native):** `ensure_link` / unlink / exclusive prune, null-sink create/destroy, `sink_exists` / `find_node_id` / snapshot sink names
- **Still Pulse for now:** volume/mute, monitor gate, default sink, sink-input migrate
- **Fallback:** `PipewireCliBackend` + `pw-link` / `pactl` remain for races and emergency (`pw-cli-backend` feature)

Probes (`link_is_live`, `sink_exists`) read `RwLock<GraphView>` — no 400ms CLI TTL on the spine.

## Adding a feature

1. Extend `contract::Intent` / `AudioBackend` / `host` as needed.
2. Implement in this crate.
3. Call from the worker / `engine_handle` — never spawn CLI from `app/`.
