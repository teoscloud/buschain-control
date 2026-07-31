# Roadmap

## Done (MVP pass)

- [x] Rename to BusChain Control; standalone flake project
- [x] Modular design system
- [x] Tabbed shell (Mixer / Playback / Recording / Output / Input / Config)
- [x] Live PipeWire enumerate + volume/mute/default/move
- [x] Dynamic mixer tracks, master left, assign apps/inputs, listen, solo/mute/gain
- [x] PluginHost + LADSPA/LV2/CLAP scan; optional discardable VST3 feature
- [x] Built-in BusChain DSP suite
- [x] Session JSON save/load
- [x] ANR fix (paced repaint + async worker)

## Done (daemon / sessions)

- [x] Named sessions + soft HW rebind
- [x] Unix JSON IPC (`ctl` / waybar / GTK mixer)
- [x] `buschain-ctl` + waybar helper
- [x] Hide-on-close UI + playback popup overlay
- [x] Flake packages + home module
- [x] App icon wired into window + .desktop

## Done (monolithic tray)

- [x] In-process graph ownership (no auto-attach thin client)
- [x] Embedded IPC inside tray UI
- [x] StatusNotifier tray (left-click popup · Open full app / Hide / Quit)
- [x] Hyprland `exec-once = buschain-control --hidden`
- [x] Drop systemd `buschain-daemon` user unit

## Done (in-process DSP host)

- [x] RT-safe insert host (`engine/src/host/`) — Rack / Slot / LADSPA / SPSC controls
- [x] `PwFxNode` — libpipewire filter, `process()` in PW RT callback
- [x] Sealed path: `{bus}.monitor → buschain_fx_* → buschain_post_* → dest`
- [x] Gen-swap structural edits (no A/B helpers, no `pipewire -c`)
- [x] Click-free bypass fades, denormals FTZ/DAZ, xrun counters, latency publish
- [x] Retire Props FX / `.sig` / dual-helper A/B cutover

## Desktop popup

- [x] Popup order: Quickshell → GTK layer-shell → egui last resort
- [ ] GTK scroll strip remains opt-in (`SCROLL_STRIP=1`); QS strip for Quant

## Done (DAW apex v2)

- [x] **F2** Native levels/mute/gate + Metadata defaults + registry snapshot/sources; PulseCompat for stream-move only; app levels/defaults via engine
- [x] **G** CLAP/VST3 on host path + insert_map + `state_blob` (always built-in; full CLAP/VST3 process APIs still incomplete)
- [x] **H** Master-bus PDC delay lines + latency publish bookkeeping; UI latency helpers
- [x] **I** Remote `AudioProcessor` hook (`host/remote.rs`, `BUSCHAIN_SANDBOX_PLUGINS`)
- [x] **J** Host pre/post meter atomics; freeze/bounce offline; UI bridge stub; ControlMsg sample timestamps
