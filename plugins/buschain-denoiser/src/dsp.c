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

/* Decision-directed α (Ephraim–Malah / Cappe). Heavy memory → less musical noise. */
#define DN_DD_ALPHA 0.98f
/* Faster α on rising SNR so speech onsets aren't swallowed (DD known weakness). */
#define DN_DD_ALPHA_ATTACK 0.72f
/* Floor on a priori SNR (~ −25 dB) — prevents hard zeros. */
#define DN_XI_MIN 0.003162f
/* MCRA: bias-compensated minima (Martin) + δ threshold (Cohen ~5). */
#define DN_MCRA_BIAS 4.5f
#define DN_MCRA_DELTA 5.5f
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
  Biquad hpf[2];
  Biquad lpf[2];
  float env[BUSCHAIN_DN_BANDS]; /* amplitude envelope (expander / meters) */
} Channel;

/* Per-band spectral-gate state (shared L/R after stereo link). */
typedef struct {
  float Sf;       /* short-term smoothed power (periodogram) */
  float Smin;     /* local minimum power */
  float Stmp;     /* temporary min (IMCRA/MCRA dual buffer) */
  float lambda_d; /* noise power estimate λd */
  float p_sp;     /* speech presence probability [0,1] */
  float xi;       /* a priori SNR ξ */
  float G_prev;   /* previous gain (decision-directed) */
  float Y2_prev;  /* previous band power */
  int   min_ctr;  /* samples until min-buffer promote */
} BandState;

struct BuschainDnState {
  double sr;
  BuschainDnParams p;
  Channel ch[2];
  BandState band[BUSCHAIN_DN_BANDS];
  float xover_hz[BUSCHAIN_DN_BANDS - 1];
  float att_coeff;
  float rel_coeff;
  float beta_s;       /* local-energy smoother */
  float alpha_p;      /* SPP smoother */
  float alpha_d;      /* noise update when speech-absent */
  int   min_win;      /* minima promote period (samples) */
  float hold[BUSCHAIN_DN_BANDS];
  float g_sm_l[BUSCHAIN_DN_BANDS]; /* temporal NR gain smooth (L) */
  float g_sm_r[BUSCHAIN_DN_BANDS]; /* temporal NR gain smooth (R) */
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
  p->freq_low_hz = 0.0f;
  p->freq_high_hz = 0.0f;
  p->hf_bias = 0.35f;
  p->stereo_link = 0.85f;
  p->stability = 0.45f; /* mid: less flicker without heavy lag */
  p->attack_ms = 1.2f;
  p->release_ms = 90.0f;
  p->knee_db = 4.0f;
  p->ratio = 12.0f;
  p->oversub = 2.2f;
  p->gate_enable = 0; /* off: spectral NR must not act as a level gate */
  p->gate_mix = 1.0f;
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
    for (int i = 0; i < BUSCHAIN_DN_BANDS - 1; i++)
      s->xover_hz[i] = sqrtf(c[i] * c[i + 1]);
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

static void band_state_reset(BandState *b) {
  /* λd starts near typical mic hiss. Minima buffers start HIGH so the first
   * search window can fall to the true noise floor (low init permanently
   * looked like “speech” via Sf/Smin). */
  float n0 = db_to_lin(-50.0f);
  float p0 = n0 * n0;
  b->Sf = p0;
  b->Smin = 1.0f;
  b->Stmp = 1.0f;
  b->lambda_d = p0;
  b->p_sp = 0.0f;
  b->xi = DN_XI_MIN;
  b->G_prev = 1.0f;
  b->Y2_prev = p0;
  b->min_ctr = 0;
}

static void recompute_time_consts(BuschainDnState *s) {
  s->att_coeff = ms_to_coeff(s->p.attack_ms, s->sr);
  s->rel_coeff = ms_to_coeff(s->p.release_ms, s->sr);
  /* Local energy ~25 ms (MCRA Sf). */
  s->beta_s = ms_to_coeff(25.0f, s->sr);
  /* SPP temporal smooth ~40 ms. */
  s->alpha_p = ms_to_coeff(40.0f, s->sr);
  /* Noise recursive average when speech-absent ~60 ms (track hiss quickly). */
  s->alpha_d = ms_to_coeff(60.0f, s->sr);
  /* Minima window ≈ 0.5 s (Cohen: 0.5–1.5 s). */
  s->min_win = (int)fmaxf(256.0f, (float)(s->sr * 0.5));
}

void buschain_dn_reset(BuschainDnState *s, double sample_rate) {
  if (!s) return;
  s->sr = sample_rate > 1.0 ? sample_rate : 48000.0;
  channel_clear(&s->ch[0]);
  channel_clear(&s->ch[1]);
  for (int i = 0; i < BUSCHAIN_DN_BANDS; i++) {
    s->gr_min[i] = 1.0f;
    s->gr_max[i] = 0.0f;
    s->hold[i] = 0.0f;
    s->g_sm_l[i] = 1.0f;
    s->g_sm_r[i] = 1.0f;
    band_state_reset(&s->band[i]);
  }
  s->in_peak = 0.0f;
  s->in_min = 1.0f;
  recompute_time_consts(s);
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
  recompute_time_consts(s);
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

void buschain_dn_debug_band(const BuschainDnState *s, int band,
                            float *Sf, float *Smin, float *lambda_d,
                            float *p_sp, float *xi, float *G) {
  if (!s || band < 0 || band >= BUSCHAIN_DN_BANDS) return;
  const BandState *b = &s->band[band];
  if (Sf) *Sf = b->Sf;
  if (Smin) *Smin = b->Smin;
  if (lambda_d) *lambda_d = b->lambda_d;
  if (p_sp) *p_sp = b->p_sp;
  if (xi) *xi = b->xi;
  if (G) *G = b->G_prev;
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

/* ---- MCRA noise update (Cohen & Berdugo, adapted to filterbank bands) ---- */
static void mcra_update(BandState *b, float Y2, float beta_s, float alpha_p,
                        float alpha_d, int min_win) {
  /* Local energy Sf (eq. 7). */
  b->Sf = beta_s * b->Sf + (1.0f - beta_s) * Y2;

  /* Dual-buffer minima (Cohen/Martin): Stmp tracks min in the window;
   * on promote, Smin ← Stmp and Stmp resets to *current* Sf so the next
   * window can follow a rising noise floor. */
  if (b->Sf < b->Smin) b->Smin = b->Sf;
  if (b->Sf < b->Stmp) b->Stmp = b->Sf;
  if (++b->min_ctr >= min_win) {
    b->Smin = b->Stmp;
    b->Stmp = b->Sf; /* reset to current — allows upward noise tracking */
    b->min_ctr = 0;
  }

  /* Speech indicator: Sf / (B·Smin) vs δ. Bias B compensates minima
   * underestimation so hiss dips don't look like perpetual speech. */
  float ratio = b->Sf / fmaxf(b->Smin * DN_MCRA_BIAS, BUSCHAIN_DN_EPS);
  float I = ratio > DN_MCRA_DELTA ? 1.0f : 0.0f;
  /* Faster presence release than attack — clear hangover between syllables. */
  float ap = (I < 0.5f) ? (alpha_p * alpha_p) : alpha_p; /* ~ quicker off */
  b->p_sp = ap * b->p_sp + (1.0f - ap) * I;

  /* Noise update: freeze hard while speech is indicated (I=1) OR smoothed
   * presence is high. Soft α̃ during onsets was letting λd learn the speech
   * itself — killing a priori SNR for the whole burst. */
  if (I < 0.5f && b->p_sp < 0.25f) {
    b->lambda_d = alpha_d * b->lambda_d + (1.0f - alpha_d) * Y2;
  }

  /* Sanity: noise power cannot exceed current local energy. */
  if (b->lambda_d > b->Sf) b->lambda_d = b->Sf;

  /* Floor: never let noise estimate collapse below a tiny fraction of Sf. */
  float floor = b->Sf * 1.0e-4f;
  if (b->lambda_d < floor) b->lambda_d = floor;
}

/* Ephraim–Malah decision-directed a priori SNR (Cappe form). */
static float decision_directed_xi(BandState *b, float Y2, float lambda_d_eff) {
  float gamma = Y2 / fmaxf(lambda_d_eff, BUSCHAIN_DN_EPS); /* a posteriori */
  float prev = (b->G_prev * b->G_prev) * b->Y2_prev / fmaxf(lambda_d_eff, BUSCHAIN_DN_EPS);
  /* Adaptive α: fast attack on rises; faster release when clearly noise
   * (classic DD hangover otherwise leaves the gate open between syllables). */
  float alpha = DN_DD_ALPHA;
  if (gamma > 1.8f * (b->xi + 1.0f))
    alpha = DN_DD_ALPHA_ATTACK;
  else if (gamma < 1.2f && b->p_sp < 0.35f)
    alpha = 0.88f;
  float xi = alpha * prev + (1.0f - alpha) * fmaxf(gamma - 1.0f, 0.0f);
  if (xi < DN_XI_MIN) xi = DN_XI_MIN;
  b->xi = xi;
  return xi;
}

/* Soft Wiener + OM-LSA speech-presence blend (Cohen).
 * G_H1 = ξ/(ξ+1); G = G_H1^p · G_min^(1−p). */
static float omlsa_gain(float xi, float p_sp, float g_min) {
  float g_h1 = xi / (xi + 1.0f);
  float p = clampf(p_sp, 0.0f, 1.0f);
  float lg = p * logf(fmaxf(g_h1, BUSCHAIN_DN_EPS))
           + (1.0f - p) * logf(fmaxf(g_min, BUSCHAIN_DN_EPS));
  float g = expf(lg);
  if (g < g_min) g = g_min;
  if (g > 1.0f) g = 1.0f;
  return g;
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
  const float oversub = clampf(s->p.oversub > 0.0f ? s->p.oversub : 2.2f, 1.0f, 6.0f);
  const float att = s->att_coeff;
  const float rel = s->rel_coeff;
  const float beta_s = s->beta_s;
  const float alpha_p = s->alpha_p;
  const float alpha_d = s->alpha_d;
  const int min_win = s->min_win;
  const float stab = clampf(s->p.stability, 0.0f, 1.0f);
  /* Higher stability → longer open-hold + slower NR gain (less flicker). */
  const float hold_samps = (0.008f + stab * 0.045f) * (float)s->sr;
  const float g_tau_ms = 1.5f + stab * 160.0f;
  const float g_smooth = ms_to_coeff(g_tau_ms, s->sr);
  /* Prefer damping upward chirps (musical noise) more than closing. */
  const float g_smooth_up = ms_to_coeff(g_tau_ms * (1.0f + stab * 0.85f), s->sr);
  const float band_w = 0.06f + stab * 0.22f;
  const int stereo = (in_r != NULL && out_r != NULL);
  const int gate_on = s->p.gate_enable != 0;
  const float gate_mix = clampf(s->p.gate_mix, 0.0f, 1.0f);

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

    float g_raw[BUSCHAIN_DN_BANDS];
    float g_l[BUSCHAIN_DN_BANDS];
    float g_r[BUSCHAIN_DN_BANDS];

    for (int b = 0; b < BUSCHAIN_DN_BANDS; b++) {
      float el_prev = s->ch[0].env[b];
      float er_prev = s->ch[1].env[b];
      float ax_l = fabsf(bands_l[b]);
      float ax_r = fabsf(bands_r[b]);

      /* Program-dependent amplitude follow (fast open). */
      float rel_l = ax_l > el_prev * 1.15f ? att : rel;
      float rel_r = ax_r > er_prev * 1.15f ? att : rel;
      follow(&s->ch[0].env[b], bands_l[b], att, rel_l);
      follow(&s->ch[1].env[b], bands_r[b], att, rel_r);

      float el = s->ch[0].env[b];
      float er = s->ch[1].env[b];
      if (link > 0.0f) {
        float mx = el > er ? el : er;
        el = el + link * (mx - el);
        er = er + link * (mx - er);
      }

      /* Linked power observation for noise / SNR (stereo max = conservative). */
      float Y2 = el * el;
      float Y2r = er * er;
      if (Y2r > Y2) Y2 = Y2r;

      BandState *bs = &s->band[b];
      mcra_update(bs, Y2, beta_s, alpha_p, alpha_d, min_win);

      /* Oversub only while speech-absent — don't poison SNR during voice. */
      float os = 1.0f + (oversub - 1.0f) * (1.0f - bs->p_sp);
      float lambda_eff = bs->lambda_d * os;

      float xi = decision_directed_xi(bs, Y2, lambda_eff);
      float range = clampf(s->p.range_db[b], 0.0f, BUSCHAIN_DN_RANGE_MAX);
      float g_min = db_to_lin(-range);
      float gamma_inst = Y2 / fmaxf(lambda_eff, BUSCHAIN_DN_EPS);

      /* Spectral NR floor from DD SNR + presence (only used when closed). */
      float g_nr = omlsa_gain(xi, bs->p_sp, g_min);
      float g_wiener = xi / (xi + 1.0f);
      if (gamma_inst > 4.0f && bs->Sf > bs->Smin * 1.8f)
        g_nr = fmaxf(g_nr, g_wiener);

      /* Openness: when signal is present, gain must be exactly unity.
       * Denoiser must never act as a program-level fader. */
      float open = clampf(bs->p_sp, 0.0f, 1.0f);
      if (gamma_inst > 2.5f)
        open = fmaxf(open, clampf((gamma_inst - 2.5f) / 3.5f, 0.0f, 1.0f));
      if (bs->Sf > bs->Smin * 2.2f)
        open = fmaxf(open, 0.90f);
      if (ax_l > el_prev * 1.8f + 1.0e-5f || ax_r > er_prev * 1.8f + 1.0e-5f)
        open = 1.0f;
      if (range <= 0.05f)
        open = 1.0f;

      float g_auto = open + (1.0f - open) * g_nr;
      if (open >= 0.88f)
        g_auto = 1.0f;

      /* Master Threshold: NR only digs below this level (soft knee).
       * Raise toward 0 → more sensitive; lower → only hush the floor. */
      float thr_b = thr_band[b];
      const float thr_knee = 8.0f;
      const float thr_half = thr_knee * 0.5f;
      float dig_l, dig_r;
      {
        float over = thr_b - lin_to_db(el); /* + when below threshold */
        if (over <= -thr_half) dig_l = 0.0f;
        else if (over >= thr_half) dig_l = 1.0f;
        else {
          float x = over + thr_half;
          dig_l = (x * x) / (thr_knee * thr_knee);
        }
        over = thr_b - lin_to_db(er);
        if (over <= -thr_half) dig_r = 0.0f;
        else if (over >= thr_half) dig_r = 1.0f;
        else {
          float x = over + thr_half;
          dig_r = (x * x) / (thr_knee * thr_knee);
        }
      }

      float g_spec_l = 1.0f + dig_l * (g_auto - 1.0f);
      float g_spec_r = 1.0f + dig_r * (g_auto - 1.0f);
      float g_spec = 0.5f * (g_spec_l + g_spec_r);

      float gl = g_spec_l;
      float gr = g_spec_r;

      /* Optional level gate (expander) — off by default; mix = dry/wet. */
      if (gate_on && gate_mix > 1.0e-4f && range > 0.05f && open < 0.88f) {
        float g_exp_l =
            expander_gain(lin_to_db(el), thr_band[b], range, s->p.knee_db, s->p.ratio);
        float g_exp_r =
            expander_gain(lin_to_db(er), thr_band[b], range, s->p.knee_db, s->p.ratio);
        float g_wet_l = gl < g_exp_l ? gl : g_exp_l;
        float g_wet_r = gr < g_exp_r ? gr : g_exp_r;
        gl = g_spec * (1.0f - gate_mix) + g_wet_l * gate_mix;
        gr = g_spec * (1.0f - gate_mix) + g_wet_r * gate_mix;
      }

      /* Open-hold at unity (not 0.90 — that was a permanent ~1 dB suck). */
      if (gl >= 0.98f && gr >= 0.98f)
        s->hold[b] = hold_samps;
      if (s->hold[b] > 0.0f) {
        s->hold[b] -= 1.0f;
        gl = 1.0f;
        gr = 1.0f;
      }

      if (gl > 0.995f) gl = 1.0f;
      if (gr > 0.995f) gr = 1.0f;

      g_raw[b] = 0.5f * (gl + gr);
      g_l[b] = gl;
      g_r[b] = gr;

      bs->G_prev = g_raw[b];
      bs->Y2_prev = Y2;
    }

    /* Coherence protect: keep open bands at unity; lift closed neighbors
     * so LR crossovers don't cancel, without attenuating the open path. */
    float g_open = 0.0f;
    for (int b = 0; b < BUSCHAIN_DN_BANDS; b++) {
      float gb = 0.5f * (g_l[b] + g_r[b]);
      if (gb > g_open) g_open = gb;
    }
    if (g_open >= 0.98f) {
      for (int b = 0; b < BUSCHAIN_DN_BANDS; b++) {
        if (g_l[b] >= 0.98f)
          g_l[b] = 1.0f;
        else if (g_l[b] < 0.55f)
          g_l[b] = g_l[b] * 0.40f + 0.55f * 0.60f;
        if (g_r[b] >= 0.98f)
          g_r[b] = 1.0f;
        else if (g_r[b] < 0.55f)
          g_r[b] = g_r[b] * 0.40f + 0.55f * 0.60f;
      }
    }

    /* Neighbor smooth: never pull an open (unity) band down. */
    float g_nb_l[BUSCHAIN_DN_BANDS];
    float g_nb_r[BUSCHAIN_DN_BANDS];
    const float w_n = band_w;
    const float w_c = 1.0f - 2.0f * band_w;
    for (int b = 0; b < BUSCHAIN_DN_BANDS; b++) {
      float left_l = b > 0 ? g_l[b - 1] : g_l[b];
      float right_l = b < BUSCHAIN_DN_BANDS - 1 ? g_l[b + 1] : g_l[b];
      float left_r = b > 0 ? g_r[b - 1] : g_r[b];
      float right_r = b < BUSCHAIN_DN_BANDS - 1 ? g_r[b + 1] : g_r[b];
      float sm_l = w_n * left_l + w_c * g_l[b] + w_n * right_l;
      float sm_r = w_n * left_r + w_c * g_r[b] + w_n * right_r;
      g_nb_l[b] = (g_l[b] >= 0.98f) ? 1.0f : sm_l;
      g_nb_r[b] = (g_r[b] >= 0.98f) ? 1.0f : sm_r;
    }

    float yl = 0.0f, yr = 0.0f;
    for (int b = 0; b < BUSCHAIN_DN_BANDS; b++) {
      float gl = g_nb_l[b];
      float gr = g_nb_r[b];
      /* Temporal smooth — Stability knob. Fast to full-open; slow chirps/flicker. */
      if (gl >= 0.995f) {
        s->g_sm_l[b] = 1.0f;
        gl = 1.0f;
      } else {
        float c = (gl > s->g_sm_l[b]) ? g_smooth_up : g_smooth;
        s->g_sm_l[b] = c * s->g_sm_l[b] + (1.0f - c) * gl;
        gl = s->g_sm_l[b];
      }
      if (gr >= 0.995f) {
        s->g_sm_r[b] = 1.0f;
        gr = 1.0f;
      } else {
        float c = (gr > s->g_sm_r[b]) ? g_smooth_up : g_smooth;
        s->g_sm_r[b] = c * s->g_sm_r[b] + (1.0f - c) * gr;
        gr = s->g_sm_r[b];
      }
      if (gl > 0.995f) gl = 1.0f;
      if (gr > 0.995f) gr = 1.0f;
      if (gl < s->gr_min[b]) s->gr_min[b] = gl;
      if (gr < s->gr_min[b]) s->gr_min[b] = gr;
      if (gl > s->gr_max[b]) s->gr_max[b] = gl;
      if (gr > s->gr_max[b]) s->gr_max[b] = gr;
      yl += bands_l[b] * gl;
      yr += bands_r[b] * gr;
      s->band[b].G_prev = 0.5f * (gl + gr);
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
