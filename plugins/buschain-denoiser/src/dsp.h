/* buschain_dn — zero-latency multiband spectral-gate denoiser
 *
 * Time-domain Linkwitz–Riley filterbank (no STFT latency). Per band:
 *   • MCRA noise-power tracking (Cohen & Berdugo) with speech-presence
 *   • Ephraim–Malah decision-directed a priori SNR (α≈0.98) — Cappe’s
 *     analysis: this is what suppresses musical noise
 *   • Soft Wiener / OM-LSA-style gain with speech-presence blend
 *     (open / present signal stays at unity — no program gain change)
 *   • Neighbor-band gain smoothing (musical-noise post-filter)
 *   • Stability control — temporal gain smooth vs NR flicker / musical noise
 *   • Optional level expander gate (Gate Enable) with knee/ratio/A/R
 *   • Transient pass-through + short unity hold
 */
#ifndef BUSCHAIN_DN_DSP_H
#define BUSCHAIN_DN_DSP_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define BUSCHAIN_DN_BANDS 6
#define BUSCHAIN_DN_RANGE_MAX 48.0f

typedef struct {
  float threshold_db;   /* 0 .. -140  dig below this level; above = unity (NR floor) */
  float range_db[BUSCHAIN_DN_BANDS]; /* 0 .. 48  max reduction per band */
  float center_hz[BUSCHAIN_DN_BANDS]; /* band center frequencies (Hz), must increase */
  float freq_low_hz;    /* edge HPF (derived from centers if 0) */
  float freq_high_hz;   /* edge LPF (derived from centers if 0) */
  float hf_bias;        /* 0..1  pass HF harmonics/transients more easily */
  float stereo_link;    /* 0..1  0=independent, 1=fully linked envelopes */
  float stability;      /* 0..1  tames NR flicker (gain smooth + neighbor mix) */
  float attack_ms;      /* envelope attack */
  float release_ms;     /* envelope release */
  float knee_db;        /* soft knee width (gate expander) */
  float ratio;          /* gate expander ratio (>= 1) */
  float oversub;        /* noise inflation for SNR (1..4 typical) */
  int   gate_enable;    /* 0 = spectral NR only; 1 = + level expander gate */
  float gate_mix;       /* 0 = dry (no gate dig) .. 1 = full gate */
  int   hpf_enable;     /* cut below freq_low */
  int   lpf_enable;     /* cut above freq_high */
  int   bypass;
} BuschainDnParams;

typedef struct BuschainDnState BuschainDnState;

BuschainDnState *buschain_dn_create(void);
void buschain_dn_destroy(BuschainDnState *s);
void buschain_dn_reset(BuschainDnState *s, double sample_rate);
void buschain_dn_set_params(BuschainDnState *s, const BuschainDnParams *p);

void buschain_dn_process(BuschainDnState *s,
                  const float *in_l, const float *in_r,
                  float *out_l, float *out_r,
                  uint32_t n_samples);

float buschain_dn_meter_input_peak_db(const BuschainDnState *s);
float buschain_dn_meter_input_min_db(const BuschainDnState *s);
float buschain_dn_meter_gr_min_db(const BuschainDnState *s, int band);
float buschain_dn_meter_gr_max_db(const BuschainDnState *s, int band);

void buschain_dn_default_params(BuschainDnParams *p);

/* Debug snapshot of one band’s gate state (tests / tuning). */
void buschain_dn_debug_band(const BuschainDnState *s, int band,
                            float *Sf, float *Smin, float *lambda_d,
                            float *p_sp, float *xi, float *G);

#ifdef __cplusplus
}
#endif

#endif /* BUSCHAIN_DN_DSP_H */
