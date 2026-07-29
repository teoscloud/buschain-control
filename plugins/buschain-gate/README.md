# BusChain Gate

Standalone stereo noise gate (LADSPA + LV2). Split from the denoiser so it can
sit on any mixer track or host independently of **BusChain Denoiser**.

## Build

```bash
make
make install   # ~/.local/lib/{ladspa,lv2}
```

## Defaults

Tuned for post-denoise mic idle (~−80 dBFS residual): threshold −78 dB, hyst 3 dB,
attack 2 ms, hold 80 ms, release 120 ms, range 100 dB.
