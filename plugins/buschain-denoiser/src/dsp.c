#include "dsp.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

#ifndef M_SQRT1_2
#define M_SQRT1_2 0.70710678118654752440
#endif

#define BUSCHAIN_DN_EPS 1.0e-12f
#define BUSCHAIN_DN_DB_MIN -140.0f

typedef struct {
  float b0, b1, b2, a1, a2;
  float z1, z2;
} Biquad;

typedef struct {
  Biquad lp[2];
  Biquad hp[2];
} Xover;

typedef struct {
  Xover x[BUSCHAIN_DN_BANDS - 1];
  Biquad hpf[2]; /* cascaded for steeper edge */
  Biquad lpf[2];
  float env[BUSCHAIN_DN_BANDS];
} Channel;

struct BuschainDnState {
  double sr;
  BuschainDnParams p;
  Channel ch[2];
  float xover_hz[BUSCHAIN_DN_BANDS - 1];
  float att_coeff;
  float rel_coeff;
  float in_peak;
  float in_min;
  float gr_min[BUSCHAIN_DN_BANDS];
  float gr_max[BUSCHAIN_DN_BANDS];
  int configured;
};

static inline float clampf(float x, float lo, float hi) {
  return x < lo ? lo : (x > hi ? hi : x);
}

static inline float db_to_lin(float db) {
  return powf(10.0f, db * 0.05f);
}

static inline float lin_to_db(float lin) {
  return 20.0f * log10f(fmaxf(lin, BUSCHAIN_DN_EPS));
}

static inline float ms_to_coeff(float ms, double sr) {
  if (ms <= 0.0f) return 0.0f;
  return expf(-1.0f / (0.001f * ms * (float)sr));
}

static void biquad_clear(Biquad *b) {
  b->z1 = b->z2 = 0.0f;
}

static inline float biquad_process(Biquad *b, float x) {
  float y = b->b0 * x + b->z1;
  b->z1 = b->b1 * x - b->a1 * y + b->z2;
  b->z2 = b->b2 * x - b->a2 * y;
  return y;
}

static void biquad_butter_lp(Biquad *b, float freq, double sr) {
  float w0 = 2.0f * (float)M_PI * clampf(freq, 10.0f, (float)sr * 0.45f) / (float)sr;
  float cosw = cosf(w0);
  float sinw = sinf(w0);
  float alpha = sinw * (float)M_SQRT1_2;
  float a0 = 1.0f + alpha;
  b->b0 = ((1.0f - cosw) * 0.5f) / a0;
  b->b1 = (1.0f - cosw) / a0;
  b->b2 = b->b0;
  b->a1 = (-2.0f * cosw) / a0;
  b->a2 = (1.0f - alpha) / a0;
}

static void biquad_butter_hp(Biquad *b, float freq, double sr) {
  float w0 = 2.0f * (float)M_PI * clampf(freq, 10.0f, (float)sr * 0.45f) / (float)sr;
  float cosw = cosf(w0);
  float sinw = sinf(w0);
  float alpha = sinw * (float)M_SQRT1_2;
  float a0 = 1.0f + alpha;
  b->b0 = ((1.0f + cosw) * 0.5f) / a0;
  b->b1 = (-(1.0f + cosw)) / a0;
  b->b2 = b->b0;
  b->a1 = (-2.0f * cosw) / a0;
  b->a2 = (1.0f - alpha) / a0;
}

static void xover_set(Xover *x, float freq, double sr) {
  biquad_butter_lp(&x->lp[0], freq, sr);
  biquad_butter_lp(&x->lp[1], freq, sr);
  biquad_butter_hp(&x->hp[0], freq, sr);
  biquad_butter_hp(&x->hp[1], freq, sr);
}

static void xover_clear(Xover *x) {
  for (int i = 0; i < 2; i++) {
    biquad_clear(&x->lp[i]);
    biquad_clear(&x->hp[i]);
  }
}

static inline float xover_lp(Xover *x, float in) {
  return biquad_process(&x->lp[1], biquad_process(&x->lp[0], in));
}

static inline float xover_hp(Xover *x, float in) {
  return biquad_process(&x->hp[1], biquad_process(&x->hp[0], in));
}

static void channel_clear(Channel *c) {
  for (int i = 0; i < BUSCHAIN_DN_BANDS - 1; i++)
    xover_clear(&c->x[i]);
  for (int i = 0; i < 2; i++) {
    biquad_clear(&c->hpf[i]);
    biquad_clear(&c->lpf[i]);
  }
  for (int i = 0; i < BUSCHAIN_DN_BANDS; i++)
    c->env[i] = 0.0f;
}

static void channel_set_filters(Channel *c, const float *xover_hz, float lo, float hi, double sr) {
  for (int i = 0; i < BUSCHAIN_DN_BANDS - 1; i++)
    xover_set(&c->x[i], xover_hz[i], sr);
  for (int i = 0; i < 2; i++) {
    biquad_butter_hp(&c->hpf[i], lo, sr);
    biquad_butter_lp(&c->lpf[i], hi, sr);
  }
}

void buschain_dn_default_params(BuschainDnParams *p) {
  /* Default user preset. Band ranges stored as magnitude (positive dB). */
  p->threshold_db = -69.0f;
  p->range_db[0] = 0.8f;
  p->range_db[1] = 0.8f;
  p->range_db[2] = 6.1f;
  p->range_db[3] = 11.4f;
  p->range_db[4] = 5.5f;
  p->range_db[5] = 5.5f;
  p->center_hz[0] = 250.0f;
  p->center_hz[1] = 574.0f;
  p->center_hz[2] = 1300.0f;
  p->center_hz[3] = 3000.0f;
  p->center_hz[4] = 7000.0f;
  p->center_hz[5] = 16000.0f;
  p->freq_low_hz = 0.0f;   /* derive from centers */
  p->freq_high_hz = 0.0f;
  p->hf_bias = 0.35f;
  p->stereo_link = 0.85f;
  p->attack_ms = 2.5f;
  p->release_ms = 120.0f;
  p->knee_db = 6.0f;
  p->ratio = 4.0f;
  p->hpf_enable = 0;
  p->lpf_enable = 0;
  p->bypass = 0;
}

BuschainDnState *buschain_dn_create(void) {
  BuschainDnState *s = (BuschainDnState *)calloc(1, sizeof(BuschainDnState));
  if (!s) return NULL;
  buschain_dn_default_params(&s->p);
  s->sr = 48000.0;
  s->configured = 0;
  return s;
}

void buschain_dn_destroy(BuschainDnState *s) {
  free(s);
}

static void sort_unique_centers(float c[BUSCHAIN_DN_BANDS], double sr) {
  /* Clamp, enforce strictly increasing centers. */
  float ny = (float)(sr * 0.45);
  for (int i = 0; i < BUSCHAIN_DN_BANDS; i++)
    c[i] = clampf(c[i], 20.0f, ny);
  for (int i = 1; i < BUSCHAIN_DN_BANDS; i++) {
    if (c[i] <= c[i - 1] * 1.05f)
      c[i] = c[i - 1] * 1.25f;
    if (c[i] > ny) c[i] = ny;
  }
}

static void recompute_xovers(BuschainDnState *s) {
  float c[BUSCHAIN_DN_BANDS];
  int have_centers = 1;
  for (int i = 0; i < BUSCHAIN_DN_BANDS; i++) {
    c[i] = s->p.center_hz[i];
    if (!(c[i] > 0.0f)) have_centers = 0;
  }

  float lo, hi;
  if (have_centers) {
    sort_unique_centers(c, s->sr);
    for (int i = 0; i < BUSCHAIN_DN_BANDS; i++)
      s->p.center_hz[i] = c[i];
    /* Crossovers at geometric means between adjacent centers. */
    for (int i = 0; i < BUSCHAIN_DN_BANDS - 1; i++)
      s->xover_hz[i] = sqrtf(c[i] * c[i + 1]);
    /* Edge filters just outside the outer bands. */
    lo = c[0] * c[0] / s->xover_hz[0];
    hi = c[BUSCHAIN_DN_BANDS - 1] * c[BUSCHAIN_DN_BANDS - 1] / s->xover_hz[BUSCHAIN_DN_BANDS - 2];
    lo = clampf(lo, 20.0f, 2000.0f);
    hi = clampf(hi, 2000.0f, (float)(s->sr * 0.45));
    if (s->p.freq_low_hz > 0.0f) lo = clampf(s->p.freq_low_hz, 20.0f, 2000.0f);
    if (s->p.freq_high_hz > 0.0f) hi = clampf(s->p.freq_high_hz, 2000.0f, (float)(s->sr * 0.45));
  } else {
    lo = clampf(s->p.freq_low_hz > 0.0f ? s->p.freq_low_hz : 40.0f, 20.0f, 2000.0f);
    hi = clampf(s->p.freq_high_hz > 0.0f ? s->p.freq_high_hz : 16000.0f, 2000.0f, (float)(s->sr * 0.45));
    if (hi <= lo * 1.5f) hi = lo * 1.5f;
    float log_lo = logf(lo);
    float log_hi = logf(hi);
    for (int i = 0; i < BUSCHAIN_DN_BANDS - 1; i++) {
      float t = (float)(i + 1) / (float)BUSCHAIN_DN_BANDS;
      s->xover_hz[i] = expf(log_lo + t * (log_hi - log_lo));
    }
    for (int i = 0; i < BUSCHAIN_DN_BANDS; i++) {
      float t0 = (float)i / (float)BUSCHAIN_DN_BANDS;
      float t1 = (float)(i + 1) / (float)BUSCHAIN_DN_BANDS;
      float e0 = expf(log_lo + t0 * (log_hi - log_lo));
      float e1 = expf(log_lo + t1 * (log_hi - log_lo));
      s->p.center_hz[i] = sqrtf(e0 * e1);
    }
  }

  channel_set_filters(&s->ch[0], s->xover_hz, lo, hi, s->sr);
  channel_set_filters(&s->ch[1], s->xover_hz, lo, hi, s->sr);
}

void buschain_dn_reset(BuschainDnState *s, double sample_rate) {
  if (!s) return;
  s->sr = sample_rate > 1.0 ? sample_rate : 48000.0;
  channel_clear(&s->ch[0]);
  channel_clear(&s->ch[1]);
  for (int i = 0; i < BUSCHAIN_DN_BANDS; i++) {
    s->gr_min[i] = 1.0f;
    s->gr_max[i] = 0.0f;
  }
  s->in_peak = 0.0f;
  s->in_min = 1.0f;
  s->att_coeff = ms_to_coeff(s->p.attack_ms, s->sr);
  s->rel_coeff = ms_to_coeff(s->p.release_ms, s->sr);
  recompute_xovers(s);
  s->configured = 1;
}

void buschain_dn_set_params(BuschainDnState *s, const BuschainDnParams *p) {
  if (!s || !p) return;
  int bands_changed = !s->configured;
  if (!bands_changed) {
    if (fabsf(s->p.freq_low_hz - p->freq_low_hz) > 0.5f ||
        fabsf(s->p.freq_high_hz - p->freq_high_hz) > 0.5f)
      bands_changed = 1;
    for (int i = 0; i < BUSCHAIN_DN_BANDS; i++) {
      if (fabsf(s->p.center_hz[i] - p->center_hz[i]) > 0.5f) {
        bands_changed = 1;
        break;
      }
    }
  }
  s->p = *p;
  s->att_coeff = ms_to_coeff(s->p.attack_ms, s->sr);
  s->rel_coeff = ms_to_coeff(s->p.release_ms, s->sr);
  if (bands_changed)
    recompute_xovers(s);
}

float buschain_dn_meter_input_peak_db(const BuschainDnState *s) {
  return s ? lin_to_db(s->in_peak) : BUSCHAIN_DN_DB_MIN;
}

float buschain_dn_meter_input_min_db(const BuschainDnState *s) {
  return s ? lin_to_db(s->in_min) : BUSCHAIN_DN_DB_MIN;
}

float buschain_dn_meter_gr_min_db(const BuschainDnState *s, int band) {
  if (!s || band < 0 || band >= BUSCHAIN_DN_BANDS) return 0.0f;
  return lin_to_db(s->gr_min[band]);
}

float buschain_dn_meter_gr_max_db(const BuschainDnState *s, int band) {
  if (!s || band < 0 || band >= BUSCHAIN_DN_BANDS) return 0.0f;
  return lin_to_db(s->gr_max[band]);
}

static float expander_gain(float level_db, float threshold_db, float range_db,
                           float knee_db, float ratio) {
  if (range_db <= 0.01f) return 1.0f;

  float knee = fmaxf(knee_db, 0.0f);
  float over;

  if (knee <= 0.01f) {
    over = threshold_db - level_db;
    if (over <= 0.0f) return 1.0f;
  } else {
    float half = knee * 0.5f;
    float delta = threshold_db - level_db;
    if (delta <= -half) return 1.0f;
    if (delta >= half) {
      over = delta;
    } else {
      float x = delta + half;
      over = (x * x) / (2.0f * knee);
    }
  }

  float r = fmaxf(ratio, 1.0f);
  float gr_db = over * (1.0f - 1.0f / r);
  if (gr_db > range_db) gr_db = range_db;
  return db_to_lin(-gr_db);
}

static void split_bands(Channel *c, float in, float bands[BUSCHAIN_DN_BANDS]) {
  float hi = in;
  for (int i = 0; i < BUSCHAIN_DN_BANDS - 1; i++) {
    float lo = xover_lp(&c->x[i], hi);
    hi = xover_hp(&c->x[i], hi);
    bands[i] = lo;
  }
  bands[BUSCHAIN_DN_BANDS - 1] = hi;
}

static inline void follow(float *env, float x, float att, float rel) {
  float ax = fabsf(x);
  if (ax > *env)
    *env = att * (*env) + (1.0f - att) * ax;
  else
    *env = rel * (*env) + (1.0f - rel) * ax;
}

void buschain_dn_process(BuschainDnState *s,
                  const float *in_l, const float *in_r,
                  float *out_l, float *out_r,
                  uint32_t n_samples) {
  if (!s || !in_l || !out_l) return;
  if (!s->configured) buschain_dn_reset(s, s->sr);

  s->in_peak = 0.0f;
  s->in_min = 1.0f;
  for (int b = 0; b < BUSCHAIN_DN_BANDS; b++) {
    s->gr_min[b] = 1.0f;
    s->gr_max[b] = 0.0f;
  }

  if (s->p.bypass) {
    if (out_l != in_l) memcpy(out_l, in_l, n_samples * sizeof(float));
    if (in_r && out_r && out_r != in_r) memcpy(out_r, in_r, n_samples * sizeof(float));
    return;
  }

  const float thr = clampf(s->p.threshold_db, BUSCHAIN_DN_DB_MIN, 0.0f);
  const float link = clampf(s->p.stereo_link, 0.0f, 1.0f);
  const float hf_bias = clampf(s->p.hf_bias, 0.0f, 1.0f);
  const float att = s->att_coeff;
  const float rel = s->rel_coeff;
  const int stereo = (in_r != NULL && out_r != NULL);

  /* HF bias: lower HF sensitivity so harmonics/transients pass. */
  float thr_band[BUSCHAIN_DN_BANDS];
  for (int b = 0; b < BUSCHAIN_DN_BANDS; b++) {
    float bias_amt = 0.0f;
    if (b >= 3) {
      float t = (float)(b - 2) / 3.0f;
      bias_amt = hf_bias * t * 12.0f;
    }
    thr_band[b] = thr - bias_amt;
  }

  for (uint32_t i = 0; i < n_samples; i++) {
    float xl = in_l[i];
    float xr = stereo ? in_r[i] : xl;

    if (s->p.hpf_enable) {
      xl = biquad_process(&s->ch[0].hpf[1], biquad_process(&s->ch[0].hpf[0], xl));
      if (stereo)
        xr = biquad_process(&s->ch[1].hpf[1], biquad_process(&s->ch[1].hpf[0], xr));
    }

    float abs_l = fabsf(xl);
    float abs_r = fabsf(xr);
    float peak = abs_l > abs_r ? abs_l : abs_r;
    if (peak > s->in_peak) s->in_peak = peak;
    float trough = abs_l < abs_r ? abs_l : abs_r;
    if (trough < s->in_min) s->in_min = trough;

    float bands_l[BUSCHAIN_DN_BANDS];
    float bands_r[BUSCHAIN_DN_BANDS];
    split_bands(&s->ch[0], xl, bands_l);
    if (stereo)
      split_bands(&s->ch[1], xr, bands_r);
    else
      memcpy(bands_r, bands_l, sizeof(bands_l));

    float yl = 0.0f, yr = 0.0f;
    for (int b = 0; b < BUSCHAIN_DN_BANDS; b++) {
      follow(&s->ch[0].env[b], bands_l[b], att, rel);
      follow(&s->ch[1].env[b], bands_r[b], att, rel);

      float el = s->ch[0].env[b];
      float er = s->ch[1].env[b];
      if (link > 0.0f) {
        float mx = el > er ? el : er;
        el = el + link * (mx - el);
        er = er + link * (mx - er);
      }

      float range = clampf(s->p.range_db[b], 0.0f, 24.0f);
      float gl = expander_gain(lin_to_db(el), thr_band[b], range, s->p.knee_db, s->p.ratio);
      float gr = expander_gain(lin_to_db(er), thr_band[b], range, s->p.knee_db, s->p.ratio);

      if (gl < s->gr_min[b]) s->gr_min[b] = gl;
      if (gr < s->gr_min[b]) s->gr_min[b] = gr;
      if (gl > s->gr_max[b]) s->gr_max[b] = gl;
      if (gr > s->gr_max[b]) s->gr_max[b] = gr;

      yl += bands_l[b] * gl;
      yr += bands_r[b] * gr;
    }

    if (s->p.lpf_enable) {
      yl = biquad_process(&s->ch[0].lpf[1], biquad_process(&s->ch[0].lpf[0], yl));
      if (stereo)
        yr = biquad_process(&s->ch[1].lpf[1], biquad_process(&s->ch[1].lpf[0], yr));
    }


    out_l[i] = yl;
    if (stereo) out_r[i] = yr;
  }
}
