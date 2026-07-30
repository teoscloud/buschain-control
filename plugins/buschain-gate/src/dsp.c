#include "dsp.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#define SG_EPS 1.0e-12f
#define SG_DB_MIN -140.0f

struct BuschainGateState {
  double sr;
  BuschainGateParams p;
  float env;
  float gain;       /* 0..1 linear */
  float hold_left;  /* samples remaining in hold */
  float att_coeff;
  float rel_coeff;
  int configured;
};

static inline float clampf(float x, float lo, float hi) {
  return x < lo ? lo : (x > hi ? hi : x);
}

static inline float db_to_lin(float db) {
  return powf(10.0f, db * 0.05f);
}

static inline float lin_to_db(float lin) {
  return 20.0f * log10f(fmaxf(lin, SG_EPS));
}

static inline float ms_to_coeff(float ms, double sr) {
  if (ms <= 0.0f) return 0.0f;
  return expf(-1.0f / (0.001f * ms * (float)sr));
}

static inline void follow(float *env, float det, float att, float rel) {
  float c = (det > *env) ? att : rel;
  *env = c * (*env) + (1.0f - c) * det;
}

void buschain_gate_default_params(BuschainGateParams *p) {
  if (!p) return;
  p->enable = 1;
  /* Calibrated for post-denoise PCM2902 idle ~ -80 dBFS at 100% path gain */
  p->threshold_db = -78.0f;
  p->hysteresis_db = 3.0f;
  p->attack_ms = 2.0f;
  p->hold_ms = 80.0f;
  p->release_ms = 120.0f;
  p->range_db = 100.0f;
  p->mix = 1.0f;
  p->bypass = 0;
}

BuschainGateState *buschain_gate_create(void) {
  BuschainGateState *s = (BuschainGateState *)calloc(1, sizeof(BuschainGateState));
  if (!s) return NULL;
  buschain_gate_default_params(&s->p);
  s->sr = 48000.0;
  return s;
}

void buschain_gate_destroy(BuschainGateState *s) {
  free(s);
}

void buschain_gate_reset(BuschainGateState *s, double sample_rate) {
  if (!s) return;
  s->sr = sample_rate > 1.0 ? sample_rate : 48000.0;
  s->env = 0.0f;
  s->gain = 1.0f; /* start open — closed-start silenced mic inserts */
  s->hold_left = 0.0f;
  s->att_coeff = ms_to_coeff(s->p.attack_ms, s->sr);
  s->rel_coeff = ms_to_coeff(s->p.release_ms, s->sr);
  s->configured = 1;
}

void buschain_gate_set_params(BuschainGateState *s, const BuschainGateParams *p) {
  if (!s || !p) return;
  s->p = *p;
  if (s->configured) {
    s->att_coeff = ms_to_coeff(s->p.attack_ms, s->sr);
    s->rel_coeff = ms_to_coeff(s->p.release_ms, s->sr);
  }
}

float buschain_gate_meter_gain(const BuschainGateState *s) {
  return s ? s->gain : 0.0f;
}

void buschain_gate_process(BuschainGateState *s,
                         const float *in_l, const float *in_r,
                         float *out_l, float *out_r,
                         uint32_t n_samples) {
  if (!s || !in_l || !out_l) return;
  int stereo = (in_r != NULL && out_r != NULL && in_r != in_l);

  if (s->p.bypass || !s->p.enable) {
    for (uint32_t i = 0; i < n_samples; i++) {
      out_l[i] = in_l[i];
      if (stereo) out_r[i] = in_r[i];
    }
    s->gain = 1.0f;
    return;
  }

  for (uint32_t i = 0; i < n_samples; i++) {
    float xl = in_l[i];
    float xr = stereo ? in_r[i] : xl;

    float det = fabsf(xl);
    if (stereo) {
      float ar = fabsf(xr);
      if (ar > det) det = ar;
    }
    /* Slightly slower envelope than sample peaks so hiss spikes don't chatter. */
    follow(&s->env, det, s->att_coeff, s->rel_coeff);

    float thr = clampf(s->p.threshold_db, SG_DB_MIN, 0.0f);
    float hyst = fmaxf(s->p.hysteresis_db, 0.0f);
    float open_db = thr;
    float close_db = thr - hyst;
    float level_db = lin_to_db(s->env);
    float hold_samples = fmaxf(s->p.hold_ms, 0.0f) * 0.001f * (float)s->sr;

    int want_open = (level_db >= open_db);
    if (s->gain > 0.5f) {
      if (level_db >= close_db)
        s->hold_left = hold_samples;
      else if (s->hold_left > 0.0f)
        s->hold_left -= 1.0f;
      want_open = (s->hold_left > 0.0f) || (level_db >= close_db);
    } else if (want_open) {
      s->hold_left = hold_samples;
    }

    float target = want_open ? 1.0f : db_to_lin(-clampf(s->p.range_db, 0.0f, 140.0f));
    float coeff = want_open ? s->att_coeff : s->rel_coeff;
    s->gain = coeff * s->gain + (1.0f - coeff) * target;

    float mix = clampf(s->p.mix, 0.0f, 1.0f);
    float wet_l = xl * s->gain;
    float wet_r = xr * s->gain;
    out_l[i] = xl * (1.0f - mix) + wet_l * mix;
    if (stereo) out_r[i] = xr * (1.0f - mix) + wet_r * mix;
  }
}
