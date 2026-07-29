/* BusChain Gate — stereo noise gate DSP (split from BusChain Denoiser) */
#ifndef BUSCHAIN_GATE_DSP_H
#define BUSCHAIN_GATE_DSP_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
  int   enable;
  float threshold_db;   /* open when envelope exceeds this (dBFS) */
  float hysteresis_db;  /* close below threshold - hysteresis */
  float attack_ms;
  float hold_ms;
  float release_ms;
  float range_db;       /* attenuation when closed (e.g. 100 ≈ mute) */
  int   bypass;
} BuschainGateParams;

typedef struct BuschainGateState BuschainGateState;

BuschainGateState *buschain_gate_create(void);
void buschain_gate_destroy(BuschainGateState *s);
void buschain_gate_reset(BuschainGateState *s, double sample_rate);
void buschain_gate_set_params(BuschainGateState *s, const BuschainGateParams *p);

void buschain_gate_process(BuschainGateState *s,
                         const float *in_l, const float *in_r,
                         float *out_l, float *out_r,
                         uint32_t n_samples);

float buschain_gate_meter_gain(const BuschainGateState *s); /* 0..1 open amount */
void buschain_gate_default_params(BuschainGateParams *p);

#ifdef __cplusplus
}
#endif

#endif /* BUSCHAIN_GATE_DSP_H */
