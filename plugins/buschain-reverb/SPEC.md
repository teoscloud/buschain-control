# BusChain Reverb — locked spec (Phase 0)

## Identity
- Label: `buschain_reverb`
- UniqueID: `392017`
- Format: LADSPA (stereo), hard-RT capable

## FDN budget
- Order N = 8 (Quality later may use 16)
- Max sample rate assumed at alloc: 96 kHz
- Predelay: 0–200 ms
- Longest delay line: ~1.5 s at max Size
- No heap in `run`

## Ports (names must match catalog)

### Audio
Input L/R, Output L/R

### Space (input control)
| Name | Range | Default |
|------|-------|---------|
| Bypass | 0–1 | 0 |
| Mix | 0–1 | 0.25 |
| Predelay (ms) | 0–200 | 20 |
| Size | 0.1–4 | 1 |
| Shape | 0.5–2 | 1 |
| RT60 (s) | 0.1–12 | 1.8 |
| Character | 0–1 | 0.55 |

### Structure
| Name | Range | Default |
|------|-------|---------|
| ER Level | 0–1 | 0.55 |
| ER Spread | 0–1 | 0.5 |
| Diffusion | 0–1 | 0.65 |
| Density | 0–1 | 0.7 |
| Modulation | 0–1 | 0.15 |

### Tone / mix tools
| Name | Range | Default |
|------|-------|---------|
| Decay Lo | 0.25–2 | 1 |
| Decay Hi | 0.25–2 | 0.7 |
| Wet HP (Hz) | 20–500 | 80 |
| Wet LP (Hz) | 2000–20000 | 12000 |
| Width | 0–1 | 0.85 |
| Duck Amount | 0–1 | 0 |
| Duck Release (ms) | 10–1000 | 200 |
| Freeze | 0–1 | 0 |
| Gate Time (ms) | 0–500 | 0 |

### Spatial (input control)
| Name | Range | Default | Role |
|------|-------|---------|------|
| Room Type | 0–6 stepped | 0 | Shell geometry |
| Source X | 0–1 | 0.28 | Floor U (left→right) |
| Source Y | 0–1 | 0.55 | Height |
| Source Z | 0–1 | 0.30 | Floor V (front→back) |
| Listener X | 0–1 | 0.72 | Floor U |
| Listener Y | 0–1 | 0.50 | Ear height |
| Listener Z | 0–1 | 0.70 | Floor V |
| Source Spacing | 0–1 | 0.35 | Stereo baseline width (~0 → mono) |
| Source Yaw | −180…180° | 0 | Parallel pair yaw when Face Lock off |
| Face Lock | 0–1 | 1 | Toe-in each speaker to listener |
| Listener Spacing | 0–1 | 0.35 | Interaural / head width |
| Ear Angle | 90–180° | 180 | Angle between ear facings (180 = opposite) |
| Ear Preset | 0–1 | 0 | 0 Flat 180 · 1 Human (sets spacing + angle) |

**Room Type enum:** 0 Shoebox · 1 Cylinder · 2 Barrel vault · 3 Cone roof · 4 Pyramid · 5 Dome · 6 Tunnel

`redesign()` places a **stereo speaker pair** and a **dual-ear head** (Listener XYZ = head center). ER taps use speaker→(bounce)→ear paths with ITD + ILD shadow; first taps are direct (no bounce) for audible listener moves. Wet L/R = left/right ear receive. Width remains wet M/S. Predelay remains a mix delay.

Wall heat is **viz-primary** for v1 (ER bounce energy per face in SpatialViz). Optional DSP meter ports may land later.

### Meters (output control)
RT60 Est (s), Echo Density, ER/Tail, Band T60 Lo/Mid/Hi, Wet Peak, Duck GR

## SpatialViz
- Scene: `ReverbRoomScene` + `ShellCmd` (room_type, half-extents, face_heat[6])
- Dual SPEAKER glyphs + aim arrows when Spacing > ~0; GlyphCmd `yaw` orients cones
- FACE toggle (default on); SPACE / YAW knobs; Alt+LMB yaw when unlocked; Ctrl+LMB spacing
- LMB on SPEAKER/EAR → floor-plane drag → Source center / Listener XZ
- LMB empty room → Size (Δx) / Shape (Δy); MMB orbit; scroll zoom
- Room Type dropdown beside Archetype; SRC Y / EAR Y knobs in Structure
- Heatmaps: tessellated / tinted wall panels in egui backend (wgpu denser path later)

## SpatialViz metric ids
`rt60`, `echo_density`, `er_tail_ratio`, `band_t60_lo`, `band_t60_mid`, `band_t60_hi`, `wet_peak`, `duck_gr`

## Listening / QA checklist
1. Drag speaker/listener — ER audible change, glyphs stick, no feedback zipper
2. Each Room Type — shell readable; no xruns at 48k/256
3. Heat responds to Size/RT60/ER Level and to moving source toward a wall
4. MMB orbit + LMB size still work; Wayland drag does not die
5. Session save/load restores Room Type + positions + Spacing/Yaw/Face Lock
6. Archetype switch does not leave glyphs outside the new shell (clamp XZ into floor footprint)
7. Spacing ≈ 0 collapses to mono inject / single glyph; wider Spacing → L/R inject + dual arrows
8. Face Lock on: both speakers toe-in to ear when center/ear/spacing move; unlock seeds Yaw with no jump
9. Face Lock off: Alt+LMB / YAW knob rotates pair; aim arrows follow
10. Long RT60 noise — no obvious metallic ringing
11. Size/Predelay — distance/room before tail
12. Decay Lo vs Hi — material change
13. Duck + Wet HP — usable on full bus
14. Freeze holds; Gate cuts cleanly
15. Archetype presets load without zipper clicks
16. Cinematic viz holds interactive pane feel (~30 fps target); if not, switch Essential or reduce fog slices
17. No XRuns at 48 kHz / 256 with N=8 on a typical laptop

## Build
```
make -C plugins/buschain-reverb
# or: make plugins
```
Installs as `buschain_reverb.so` on `LADSPA_PATH`.
