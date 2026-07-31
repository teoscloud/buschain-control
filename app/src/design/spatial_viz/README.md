# SpatialViz

Backend-agnostic 3D audio graphs for BusChain plugin panels.

## Add a scene (3 steps)

1. Implement `SpatialScene::build(&self, frame, metrics) -> SpatialDrawList` in a module that **does not** `use egui` / `wgpu`.
2. Use helpers (`mesh::push_box`, `mesh::push_shell`, `rays::push_polyline`, `field::push_fog`, …).
3. In the plugin panel: `let list = scene.build(...); backend.submit(&list);` plus optional `hit::pick`.

## Add a backend

Implement `SpatialBackend` (`begin_frame` / `submit` / `end_frame`) that consumes `SpatialDrawList`.  
v1: `backend::egui::EguiPainterBackend`.  
Future: `backend::wgpu` behind feature `spatial-wgpu` — scenes stay unchanged.

## ShellCmd + wall heat

`SpatialCmd::Shell` carries `room_type` (0–6, same as LADSPA Room Type), half-extents, `face_heat[6]`, material, and segment count. Scenes stay backend-agnostic; egui tessellates wire + translucent heated panels (cool→hot). Heat is derived from the same ER bounce model the room scene uses (viz-primary until optional DSP wall meters land).

## Interaction helpers

`hit::camera_ray`, `hit::intersect_floor`, `hit::pick` — LMB on Source/Listener glyphs for floor XZ drag; empty room keeps Size/Shape drag.

## Stereo speakers (reverb)

`ReverbRoomScene` uses Source center + `Source Spacing` / `Source Yaw` / `Face Lock`, plus `Listener Spacing` / `Ear Angle` for a dual-ear head. Dual Source + dual Listener glyphs with `GlyphCmd.yaw`; aim arrows and baselines. Helpers: `stereo_speakers`, `stereo_ears`, `bearing_yaw_deg`. Wall heat uses min–max + gamma contrast so concentration reads clearly.

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
