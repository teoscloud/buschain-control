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

### Meters (output control)
RT60 Est (s), Echo Density, ER/Tail, Band T60 Lo/Mid/Hi, Wet Peak, Duck GR

## SpatialViz metric ids
`rt60`, `echo_density`, `er_tail_ratio`, `band_t60_lo`, `band_t60_mid`, `band_t60_hi`, `wet_peak`, `duck_gr`

## Listening / QA checklist
1. Long RT60 noise — no obvious metallic ringing
2. Size/Predelay — distance/room before tail
3. Decay Lo vs Hi — material change
4. Duck + Wet HP — usable on full bus
5. Freeze holds; Gate cuts cleanly
6. Archetype presets load without zipper clicks
7. Cinematic viz holds interactive pane feel (~30 fps target); if not, switch Essential or reduce fog slices
8. No XRuns at 48 kHz / 256 with N=8 on a typical laptop

## Build
```
make -C plugins/buschain-reverb
# or: make plugins
```
Installs as `buschain_reverb.so` on `LADSPA_PATH`.
