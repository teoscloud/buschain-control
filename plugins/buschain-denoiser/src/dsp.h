/* buschain_dn — zero-latency multiband downward-expander denoiser DSP
 *
 * Time-domain only (no FFT), 6 bands, master threshold, per-band max
 * reduction ("range"), HF bias, stereo link.
 */
#ifndef BUSCHAIN_DN_DSP_H
#define BUSCHAIN_DN_DSP_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define BUSCHAIN_DN_BANDS 6

typedef struct {
  float threshold_db;   /* 0 .. -140  (higher = more sensitive / more NR) */
  float range_db[BUSCHAIN_DN_BANDS]; /* 0 .. 24  max reduction per band */
  float center_hz[BUSCHAIN_DN_BANDS]; /* band center frequencies (Hz), must increase */
  float freq_low_hz;    /* edge HPF (derived from centers if 0) */
  float freq_high_hz;   /* edge LPF (derived from centers if 0) */
  float hf_bias;        /* 0..1  pass HF harmonics/transients more easily */
  float stereo_link;    /* 0..1  0=independent, 1=fully linked envelopes */
  float attack_ms;      /* envelope attack */
  float release_ms;     /* envelope release */
  float knee_db;        /* soft knee width */
  float ratio;          /* expander ratio (>= 1) */
  int   hpf_enable;     /* cut below freq_low */
  int   lpf_enable;     /* cut above freq_high */
  int   bypass;
} BuschainDnParams;

typedef struct BuschainDnState BuschainDnState;

BuschainDnState *buschain_dn_create(void);
void buschain_dn_destroy(BuschainDnState *s);
void buschain_dn_reset(BuschainDnState *s, double sample_rate);
void buschain_dn_set_params(BuschainDnState *s, const BuschainDnParams *p);

/* Process interleaved stereo (LRLR…) or separate channel buffers. */
void buschain_dn_process(BuschainDnState *s,
                  const float *in_l, const float *in_r,
                  float *out_l, float *out_r,
                  uint32_t n_samples);

/* Metering helpers (dB). Call after process. */
float buschain_dn_meter_input_peak_db(const BuschainDnState *s);
float buschain_dn_meter_input_min_db(const BuschainDnState *s);
float buschain_dn_meter_gr_min_db(const BuschainDnState *s, int band); /* most reduction (more negative) */
float buschain_dn_meter_gr_max_db(const BuschainDnState *s, int band); /* least reduction (closest to 0) */

void buschain_dn_default_params(BuschainDnParams *p);

#ifdef __cplusplus
}
#endif

#endif /* BUSCHAIN_DN_DSP_H */
