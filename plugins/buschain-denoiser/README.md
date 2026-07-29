# BusChain Denoiser

Zero-latency, time-domain 6-band downward-expander denoiser.
The post-denoise **gate is not included** — use **buschain-gate** on the same track.

Builds as LADSPA + LV2 for PipeWire filter-chain, Carla, Ardour, etc.

```bash
make
make install
```

LADSPA UniqueID: `392001` · Label: `buschain_denoiser`
