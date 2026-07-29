# buschain-engine

First-class audio engine for BusChain Control. **App code must not call `pactl` / `pw-link` / `pw-cli` / `pipewire` directly** — every capability lives here as a typed contract. If a feature needs something new, extend this crate first.

## Layers

| Module | Role |
|--------|------|
| `contract` | Public intents (`EnsureBus`, `SetRoute`, `EnsureFxChain`, `ArmSession`, …) |
| `domain` | Typed node/link/clock/insert identities |
| `clock` | `GraphClock`, `PerformanceProfile`, device probe + resolve |
| `plan` | Desired state (buses, routes, bridges, **fx_chains**, **bus_egress**, `speakers_armed`) |
| `pipeline` | Sealed insert wet-switch + **arm** (dwell, disarm/arm egress) |
| `backend` | `AudioBackend` + CLI PipeWire + filter-chain spawn |
| `runtime` | `Engine` handle used by the worker |

## Clock domains

- **GraphClock** = Master HW rate + quantum after `BindMasterClock` (`PerformanceProfile`).
- **DeviceCaps** = true rates from PipeWire `EnumFormat` ∪ ALSA `/proc/asound/cardN/stream*` `Rates:` (never substring false positives like phantom 192k).
- **Custom** may only select rates/quantums in `DeviceCaps`. Invalid session values are clamped on load/refresh.
- **Apply (`BindMasterClock` / `BindDeviceClock`)**:
  1. Custom → `clock.force-rate` / `clock.force-quantum` (quantum `0` when soft); Balanced → clear forces
  2. Wait until Master HW running rate matches (ALSA momentary / PW Format — not stale pactl)
  3. **Migrate-recreate** app buses (`buschain_master` / `buschain_track_*`) at GraphClock via `buschain_hold` stream park
  4. Tear down `buschain_rs_*` + post helpers; Hotplug `ClockBind` ForceRespawns FX
- Per-device prefs live in session `device_clocks`; Output/Input panels + Settings Master HW share the same Apply UI.
- Rate bridges are **inbound only** (external → BusChain bus), exclusive (no parallel direct+bridge).
- Outbound Master→HW is never bridged (device adapter).

## Insert pipeline (sealed DAW contract)

**Plugins are black boxes. The mixer only declares the rack. The engine owns the wire.**

```
apps → buschain_track_* / buschain_master
         │
         ▼
   {bus}.monitor ──exclusive──► buschain_fx_{bus}   (n0→n1→… LADSPA chain)
                                     │
                                     ▼
                              buschain_post_{bus}   (meter tap)
                                     │
                                     ▼
                              Master / HW dest
```

| Intent | Effect | Forbidden |
|--------|--------|-----------|
| `PushFxControls` | Props on `n{i}` (power / knobs / mix) | Respawn, unlink bus, mute `{bus}.monitor` |
| `EnsureFxChain(Force)` | Respawn one chain; hold-only → dwell → arm egress | Touch other tracks; recreate app buses |
| `EnsureFxChain(Idempotent)` | No-op when signature + wet path match | Silent dry when rack non-empty |
| `ArmSession` | Cold: disarm → spines → dwell → barrier → Master→HW. Warm: if live PW already matches Desired → adopt (no disarm/ForceRespawn) | App-side `pw-link` arming |
| `BindMasterClock` | Force HW rate/quantum → wait → BusChain props → drop bridges/posts | Soft-only props without HW switch |
| `TeardownFxChain` | Stop helper for one bus | — |

Types: `InsertSlot` → `ChainSpec` → `WirePlan` → `ChainState::{Dry,Building,Wet,Failed}`.

### Sealed arming

1. **Disarm** track egress (keep `bus→buschain_hold`; optional `bus→fx` feed).
2. Spawn / feed FX with **no** `post→dest` / `bus→dest`.
3. Spine instant-ready, then **~200ms continuous dwell** (flap resets).
4. `arm_track_egress` to all `DesiredState.bus_egress` hops.
5. **Session barrier**: Master→HW stays down until Master spine ready and every unmuted insert track is Wet or Failed.
6. Latch `speakers_armed`; idle reconcile repairs Master→HW only when latched — never re-imposes the barrier.

Failed ensure → `restore_dry` + dry arm (fail-open). Failed does not block the barrier forever.

Hard rules:

- Never mute `{bus}.monitor` during wet switch (corks Chromium).
- Never return “OK dry” when the rack is non-empty — return `Failed` with spawn log.
- Keepalive `{bus}.monitor → buschain_hold` always preserved.
- Never open dry→dest while Building (cold path). Warm structural edits use **A/B cutover** (`buschain_fx_*__stg`) so the live rack stays audible until flip.
- v1 process path: **LADSPA only** (monolithic `n0→n1→…` filter-chain). Ideal later: in-process DSP host — see `docs/ROADMAP.md`.

## GraphSupervisor (`Engine::reconcile`)

Continuous Desired↔live health. Idle worker ticks + after Hotplug sync session into
`DesiredState` then call `reconcile()` / `reconcile_light()`:

- Ensure declared buses + keepalive
- Open bus gains (never leave create-mute@0)
- Idempotent FX ensure for non-empty racks
- Master→HW only when `speakers_armed` (post when wet)
- Preferred default sink watchdog (debounced)

Full Apply / launch calls `ArmSession`: **warm-adopts** when buses, FX signatures, spines, and Master→HW are already live (UI restart); otherwise sealed cold bring-up. Idempotent FX also adopts orphan `pipewire -c` helpers after process restart (`sink_exists` + signature + audible path). Nuclear teardown is **not** a user verb — Config → Advanced only.

## Live Graph rules

- **Levels / mute** — never mute or recreate app-facing buses; silence outbound monitors / FX only.
- **FX power / knobs** — `PushFxControls` only.
- **FX add/remove/reorder** — `EnsureFxChain(Force)` once per track.
- **Rate bridges** — inbound external→BusChain only; mic hops exclusive.
- **Exclusive routes** — prune by node prefix (`sink:`), not substring.
- **Custom clock** — device-capable rates only; Apply switches HW + GraphClock together.

## Adding a feature

1. Add/extend a `contract::Intent` (or backend method).
2. Implement in `backend` / `pipeline`.
3. Call from app via `Engine` / `engine_handle` — never shell out from `app/`.
