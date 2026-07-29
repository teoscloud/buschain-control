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
- [x] StatusNotifier tray (Show / Hide / Mixer / Quit)
- [x] Hyprland `exec-once = buschain-control --hidden`
- [x] Drop systemd `buschain-daemon` user unit

## Next

- [ ] Native in-process VST3 backend (replace Carla adapter)
- [ ] Real CLAP instantiate + process

