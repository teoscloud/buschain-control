# BusChain Denoiser

Zero-latency, time-domain **6-band spectral-gate** denoiser for mic hiss / room tone.

The post-denoise **gate is not included** — use **buschain-gate** on the same track.

## Why not STFT / ML?

Industry leaders (iZotope RX, LogMMSE, DeepFilterNet, RNNoise) use STFT or neural nets and **must** buffer a frame — they cannot be true zero-latency. This plugin stays sample-by-sample for PipeWire live paths and ports the *control laws* that make those systems work.

## Algorithm (research → filterbank)

| Stage | Source | Role |
| --- | --- | --- |
| MCRA noise tracking | Cohen & Berdugo, IEEE SPL 2002 | Per-band noise power via minima-controlled recursive averaging + speech presence |
| Bias-compensated minima | Martin minimum statistics | Stops hiss dips from looking like perpetual “speech” |
| Hard freeze of λd in speech | Practical fix on MCRA soft-α | Prevents the noise estimate from learning the voice itself |
| Decision-directed a priori SNR | Ephraim & Malah 1984; Cappe 1994 | α≈0.98 SNR memory — primary musical-noise killer; adaptive α on attack/release |
| Soft Wiener / OM-LSA blend | Cohen OM-LSA | `G = G_H1^p · G_min^(1−p)` with speech presence `p` |
| Neighbor-band gain smooth | Classical musical-noise PF | Spreads isolated chirps; helps LR filterbank coherence |
| Coherence protect | Filterbank-specific | When a band is clearly open, lift others to avoid tonal cancellation |
| User expander | Multiband dynamics | Threshold / range / ratio still dig on demand |
| Transient hold | Practical | Protects consonants / attacks |

## Build

```bash
make
make install
make -C .  # then: gcc -O2 -Isrc -o smoke src/smoke_test.c src/dsp.c -lm && ./smoke
```

LADSPA UniqueID: `392001` · Label: `buschain_denoiser`
