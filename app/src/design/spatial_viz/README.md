# SpatialViz

Backend-agnostic 3D audio graphs for BusChain plugin panels.

## Add a scene (3 steps)

1. Implement `SpatialScene::build(&self, frame, metrics) -> SpatialDrawList` in a module that **does not** `use egui` / `wgpu`.
2. Use helpers (`mesh::push_box`, `rays::push_polyline`, `field::push_fog`, …).
3. In the plugin panel: `let list = scene.build(...); backend.submit(&list);` plus optional `hit::pick`.

## Add a backend

Implement `SpatialBackend` (`begin_frame` / `submit` / `end_frame`) that consumes `SpatialDrawList`.  
v1: `backend::egui::EguiPainterBackend`.  
Future: `backend::wgpu` behind feature `spatial-wgpu` — scenes stay unchanged.

## Anti-hurries

- No `Painter` / `Color32` / `Shape` inside scenes
- Materials are `SpatialMaterial` data
- Explicit sort keys / z on commands — do not rely on painter order alone

## Metric ids (reverb)

`rt60`, `echo_density`, `er_tail_ratio`, `band_t60_lo|mid|hi`, `wet_peak`, `duck_gr`

## Consumers

| Plugin | Scene |
|--------|--------|
| `buschain_reverb` | `ReverbRoomScene` |
| (stub) delay | `TapFieldScene` |
