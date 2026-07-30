/* BusChain Room — hybrid ER + FDN reverb */
#ifndef BUSCHAIN_REVERB_DSP_H
#define BUSCHAIN_REVERB_DSP_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
  int   bypass;
  float mix;
  float predelay_ms;
  float size;
  float shape;
  float rt60;
  float character;
  float er_level;
  float er_spread;
  float diffusion;
  float density;
  float modulation;
  float decay_lo;
  float decay_hi;
  float wet_hp_hz;
  float wet_lp_hz;
  float width;
  float duck_amount;
  float duck_release_ms;
  float freeze;
  float gate_time_ms;
} BuschainReverbParams;

typedef struct {
  float rt60_est;
  float echo_density;
  float er_tail;
  float band_t60_lo;
  float band_t60_mid;
  float band_t60_hi;
  float wet_peak;
  float duck_gr;
} BuschainReverbMeters;

typedef struct BuschainReverbState BuschainReverbState;

BuschainReverbState *buschain_reverb_create(void);
void buschain_reverb_destroy(BuschainReverbState *s);
void buschain_reverb_reset(BuschainReverbState *s, double sample_rate);
void buschain_reverb_set_params(BuschainReverbState *s, const BuschainReverbParams *p);
void buschain_reverb_default_params(BuschainReverbParams *p);

void buschain_reverb_process(BuschainReverbState *s,
                             const float *in_l, const float *in_r,
                             float *out_l, float *out_r,
                             uint32_t n_samples);

void buschain_reverb_get_meters(const BuschainReverbState *s, BuschainReverbMeters *m);

#ifdef __cplusplus
}
#endif

#endif /* BUSCHAIN_REVERB_DSP_H */
