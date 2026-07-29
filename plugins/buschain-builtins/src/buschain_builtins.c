/* BusChain Control built-in LADSPA suite: EQ, compressor, limiter, softclip, pitch, overdrive */
#include "ladspa.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

#define N_PLUGINS 7

/* ---- shared helpers ---- */
static float clampf(float x, float lo, float hi) {
  return x < lo ? lo : (x > hi ? hi : x);
}
static float db_to_lin(float db) { return powf(10.0f, db * 0.05f); }

/* ===================== Soft Clipper (Fruity Soft Clipper–style) =====================
 * THRES = knee start (linear below).  POST = makeup after clip.
 * Piecewise (matches FL graph shape — NOT full-range tanh):
 *   |x| ≤ T  →  y = x                         (unity)
 *   |x| > T  →  y = sign(x)·(T + (1−T)·tanh((|x|−T)/(1−T)))
 *             → soft knee into a ±1.0 ceiling, then × POST.
 */
enum { SC_IN_L, SC_IN_R, SC_OUT_L, SC_OUT_R, SC_THRES, SC_POST, SC_MIX, SC_N };
typedef struct {
  const LADSPA_Data *p[SC_N];
  LADSPA_Data *o[SC_N];
} SoftClip;

static inline float sc_xfer(float x, float thres) {
  float t = clampf(thres, 0.05f, 0.999f);
  float ax = fabsf(x);
  float y;
  if (ax <= t) {
    y = ax;
  } else {
    float denom = 1.0f - t;
    float over = (ax - t) / fmaxf(denom, 1.0e-6f);
    y = t + denom * tanhf(over);
  }
  return copysignf(y, x);
}

static LADSPA_Handle sc_inst(const LADSPA_Descriptor *d, unsigned long sr) {
  (void)d; (void)sr;
  return calloc(1, sizeof(SoftClip));
}
static void sc_connect(LADSPA_Handle h, unsigned long port, LADSPA_Data *data) {
  SoftClip *s = h;
  if (port < SC_N) { s->p[port] = data; s->o[port] = data; }
}
static void sc_run(LADSPA_Handle h, unsigned long n) {
  SoftClip *s = h;
  float thres = s->p[SC_THRES] ? clampf(*s->p[SC_THRES], 0.05f, 1.0f) : 0.5f;
  float post = s->p[SC_POST] ? clampf(*s->p[SC_POST], 0.0f, 4.0f) : 1.0f;
  float mix = s->p[SC_MIX] ? clampf(*s->p[SC_MIX], 0.0f, 1.0f) : 1.0f;
  const float *il = s->p[SC_IN_L], *ir = s->p[SC_IN_R];
  float *ol = s->o[SC_OUT_L], *or_ = s->o[SC_OUT_R];
  if (!il || !ol) return;
  if (!ir) ir = il;
  if (!or_) or_ = ol;
  /* Mix≈0 or Post≈0: dry pass-through. Always write outs (in-place hosts keep ol==il). */
  if (mix < 1.0e-5f || post < 1.0e-5f) {
    for (unsigned long i = 0; i < n; i++) { ol[i] = il[i]; or_[i] = ir[i]; }
    return;
  }
  for (unsigned long i = 0; i < n; i++) {
    float wl = sc_xfer(il[i], thres) * post;
    float wr = sc_xfer(ir[i], thres) * post;
    ol[i] = il[i] + mix * (wl - il[i]);
    or_[i] = ir[i] + mix * (wr - ir[i]);
  }
}
static void sc_cleanup(LADSPA_Handle h) { free(h); }

/* ===================== Limiter ===================== */
enum { LM_IN_L, LM_IN_R, LM_OUT_L, LM_OUT_R, LM_CEIL, LM_REL, LM_BYPASS, LM_N };
typedef struct {
  const LADSPA_Data *p[LM_N];
  LADSPA_Data *o[LM_N];
  float env;
  float sr;
} Limiter;

static LADSPA_Handle lm_inst(const LADSPA_Descriptor *d, unsigned long sr) {
  (void)d;
  Limiter *s = calloc(1, sizeof(Limiter));
  if (s) s->sr = (float)sr;
  return s;
}
static void lm_connect(LADSPA_Handle h, unsigned long port, LADSPA_Data *data) {
  Limiter *s = h;
  if (port < LM_N) { s->p[port] = data; s->o[port] = data; }
}
static void lm_run(LADSPA_Handle h, unsigned long n) {
  Limiter *s = h;
  float ceil_db = s->p[LM_CEIL] ? *s->p[LM_CEIL] : -0.1f;
  float rel_ms = s->p[LM_REL] ? *s->p[LM_REL] : 50.0f;
  float ceiling = db_to_lin(ceil_db);
  float coeff = expf(-1.0f / (0.001f * fmaxf(rel_ms, 1.0f) * s->sr));
  const float *il = s->p[LM_IN_L], *ir = s->p[LM_IN_R];
  float *ol = s->o[LM_OUT_L], *or_ = s->o[LM_OUT_R];
  if (!il || !ol) return;
  if (!ir) ir = il;
  if (!or_) or_ = ol;
  if (s->p[LM_BYPASS] && *s->p[LM_BYPASS] >= 0.5f) {
    for (unsigned long i = 0; i < n; i++) { ol[i] = il[i]; or_[i] = ir[i]; }
    return;
  }
  for (unsigned long i = 0; i < n; i++) {
    float peak = fabsf(il[i]);
    float pr = fabsf(ir[i]);
    if (pr > peak) peak = pr;
    if (peak > s->env) s->env = peak;
    else s->env = coeff * s->env + (1.0f - coeff) * peak;
    float g = 1.0f;
    if (s->env > ceiling && s->env > 1e-12f)
      g = ceiling / s->env;
    ol[i] = il[i] * g;
    or_[i] = ir[i] * g;
  }
}
static void lm_cleanup(LADSPA_Handle h) { free(h); }

/* ===================== Compressor ===================== */
enum { CM_IN_L, CM_IN_R, CM_OUT_L, CM_OUT_R, CM_THR, CM_RATIO, CM_ATK, CM_REL, CM_MAKEUP, CM_BYPASS, CM_N };
typedef struct {
  const LADSPA_Data *p[CM_N];
  LADSPA_Data *o[CM_N];
  float env;
  float sr;
} Comp;

static LADSPA_Handle cm_inst(const LADSPA_Descriptor *d, unsigned long sr) {
  (void)d;
  Comp *s = calloc(1, sizeof(Comp));
  if (s) s->sr = (float)sr;
  return s;
}
static void cm_connect(LADSPA_Handle h, unsigned long port, LADSPA_Data *data) {
  Comp *s = h;
  if (port < CM_N) { s->p[port] = data; s->o[port] = data; }
}
static void cm_run(LADSPA_Handle h, unsigned long n) {
  Comp *s = h;
  float thr = s->p[CM_THR] ? *s->p[CM_THR] : -18.0f;
  float ratio = s->p[CM_RATIO] ? fmaxf(*s->p[CM_RATIO], 1.0f) : 4.0f;
  float atk = s->p[CM_ATK] ? *s->p[CM_ATK] : 10.0f;
  float rel = s->p[CM_REL] ? *s->p[CM_REL] : 100.0f;
  float makeup = db_to_lin(s->p[CM_MAKEUP] ? *s->p[CM_MAKEUP] : 0.0f);
  float ca = expf(-1.0f / (0.001f * fmaxf(atk, 0.1f) * s->sr));
  float cr = expf(-1.0f / (0.001f * fmaxf(rel, 1.0f) * s->sr));
  const float *il = s->p[CM_IN_L], *ir = s->p[CM_IN_R];
  float *ol = s->o[CM_OUT_L], *or_ = s->o[CM_OUT_R];
  if (!il || !ol) return;
  if (!ir) ir = il;
  if (!or_) or_ = ol;
  if (s->p[CM_BYPASS] && *s->p[CM_BYPASS] >= 0.5f) {
    for (unsigned long i = 0; i < n; i++) { ol[i] = il[i]; or_[i] = ir[i]; }
    return;
  }
  for (unsigned long i = 0; i < n; i++) {
    float peak = fabsf(il[i]);
    float pr = fabsf(ir[i]);
    if (pr > peak) peak = pr;
    float c = (peak > s->env) ? ca : cr;
    s->env = c * s->env + (1.0f - c) * peak;
    float level = 20.0f * log10f(fmaxf(s->env, 1e-12f));
    float gr = 1.0f;
    if (level > thr) {
      float over = level - thr;
      float out_db = thr + over / ratio;
      gr = db_to_lin(out_db - level);
    }
    ol[i] = il[i] * gr * makeup;
    or_[i] = ir[i] * gr * makeup;
  }
}
static void cm_cleanup(LADSPA_Handle h) { free(h); }

/* ===================== EQ (1 peaking + HP + LP + shelves via mode) ===================== */
enum { EQ_IN_L, EQ_IN_R, EQ_OUT_L, EQ_OUT_R, EQ_FREQ, EQ_GAIN, EQ_Q, EQ_MODE, EQ_BYPASS, EQ_N };
/* mode: 0=peak 1=lowShelf 2=highShelf 3=HP 4=LP */
typedef struct {
  const LADSPA_Data *p[EQ_N];
  LADSPA_Data *o[EQ_N];
  float b0, b1, b2, a1, a2, z1l, z2l, z1r, z2r;
  float sr;
  float last_f, last_g, last_q, last_m;
} Eq;

static void eq_design(Eq *s, float freq, float gain_db, float q, float mode) {
  float w0 = 2.0f * (float)M_PI * clampf(freq, 20.0f, s->sr * 0.45f) / s->sr;
  float cosw = cosf(w0), sinw = sinf(w0);
  float A = powf(10.0f, gain_db / 40.0f);
  q = fmaxf(q, 0.1f);
  float alpha = sinw / (2.0f * q);
  float b0, b1, b2, a0, a1, a2;
  int m = (int)mode;
  if (m == 3) { /* HP */
    b0 = (1 + cosw) * 0.5f; b1 = -(1 + cosw); b2 = b0;
    a0 = 1 + alpha; a1 = -2 * cosw; a2 = 1 - alpha;
  } else if (m == 4) { /* LP */
    b0 = (1 - cosw) * 0.5f; b1 = 1 - cosw; b2 = b0;
    a0 = 1 + alpha; a1 = -2 * cosw; a2 = 1 - alpha;
  } else if (m == 1) { /* low shelf */
    float two_sqrtA_alpha = 2.0f * sqrtf(A) * alpha;
    b0 = A * ((A + 1) - (A - 1) * cosw + two_sqrtA_alpha);
    b1 = 2 * A * ((A - 1) - (A + 1) * cosw);
    b2 = A * ((A + 1) - (A - 1) * cosw - two_sqrtA_alpha);
    a0 = (A + 1) + (A - 1) * cosw + two_sqrtA_alpha;
    a1 = -2 * ((A - 1) + (A + 1) * cosw);
    a2 = (A + 1) + (A - 1) * cosw - two_sqrtA_alpha;
  } else if (m == 2) { /* high shelf */
    float two_sqrtA_alpha = 2.0f * sqrtf(A) * alpha;
    b0 = A * ((A + 1) + (A - 1) * cosw + two_sqrtA_alpha);
    b1 = -2 * A * ((A - 1) + (A + 1) * cosw);
    b2 = A * ((A + 1) + (A - 1) * cosw - two_sqrtA_alpha);
    a0 = (A + 1) - (A - 1) * cosw + two_sqrtA_alpha;
    a1 = 2 * ((A - 1) - (A + 1) * cosw);
    a2 = (A + 1) - (A - 1) * cosw - two_sqrtA_alpha;
  } else { /* peak */
    b0 = 1 + alpha * A; b1 = -2 * cosw; b2 = 1 - alpha * A;
    a0 = 1 + alpha / A; a1 = -2 * cosw; a2 = 1 - alpha / A;
  }
  s->b0 = b0 / a0; s->b1 = b1 / a0; s->b2 = b2 / a0;
  s->a1 = a1 / a0; s->a2 = a2 / a0;
  s->last_f = freq; s->last_g = gain_db; s->last_q = q; s->last_m = mode;
}

static LADSPA_Handle eq_inst(const LADSPA_Descriptor *d, unsigned long sr) {
  (void)d;
  Eq *s = calloc(1, sizeof(Eq));
  if (s) { s->sr = (float)sr; eq_design(s, 1000.0f, 0.0f, 0.707f, 0.0f); }
  return s;
}
static void eq_connect(LADSPA_Handle h, unsigned long port, LADSPA_Data *data) {
  Eq *s = h;
  if (port < EQ_N) { s->p[port] = data; s->o[port] = data; }
}
static void eq_run(LADSPA_Handle h, unsigned long n) {
  Eq *s = h;
  float freq = s->p[EQ_FREQ] ? *s->p[EQ_FREQ] : 1000.0f;
  float gain = s->p[EQ_GAIN] ? *s->p[EQ_GAIN] : 0.0f;
  float q = s->p[EQ_Q] ? *s->p[EQ_Q] : 0.707f;
  float mode = s->p[EQ_MODE] ? *s->p[EQ_MODE] : 0.0f;
  const float *il = s->p[EQ_IN_L], *ir = s->p[EQ_IN_R];
  float *ol = s->o[EQ_OUT_L], *or_ = s->o[EQ_OUT_R];
  if (!il || !ol) return;
  if (!ir) ir = il;
  if (!or_) or_ = ol;
  if (s->p[EQ_BYPASS] && *s->p[EQ_BYPASS] >= 0.5f) {
    for (unsigned long i = 0; i < n; i++) { ol[i] = il[i]; or_[i] = ir[i]; }
    return;
  }
  if (freq != s->last_f || gain != s->last_g || q != s->last_q || mode != s->last_m)
    eq_design(s, freq, gain, q, mode);
  for (unsigned long i = 0; i < n; i++) {
    float x = il[i];
    float y = s->b0 * x + s->z1l;
    s->z1l = s->b1 * x - s->a1 * y + s->z2l;
    s->z2l = s->b2 * x - s->a2 * y;
    ol[i] = y;
    x = ir[i];
    y = s->b0 * x + s->z1r;
    s->z1r = s->b1 * x - s->a1 * y + s->z2r;
    s->z2r = s->b2 * x - s->a2 * y;
    or_[i] = y;
  }
}
static void eq_cleanup(LADSPA_Handle h) { free(h); }

/* ===================== Pitch (delay-modulation + dual-grain crossfade) =====================
 * ratio = 2^((semi + cents/100)/12);  >1 higher, <1 lower.
 * Each sample: delay += (1 − ratio).  ratio<1 grows delay → lower pitch.
 * Dual Hann readers hide wrap jumps. Smooth = grain length. */
enum {
  PT_IN_L = 0, PT_IN_R, PT_OUT_L, PT_OUT_R,
  PT_SEMITONES, PT_CENTS, PT_SMOOTH, PT_BYPASS, PT_N
};
#define PT_BUF 96000
typedef struct {
  const LADSPA_Data *p[PT_N];
  LADSPA_Data *o[PT_N];
  float buf_l[PT_BUF], buf_r[PT_BUF];
  unsigned long w;
  double delay; /* samples behind write head */
  float ratio_sm;
  float sr;
} Pitch;

static inline float pt_cub(const float *b, double pos) {
  long i1 = (long)floor(pos);
  float frac = (float)(pos - (double)i1);
  while (i1 < 0) i1 += PT_BUF;
  unsigned long u1 = (unsigned long)i1 % PT_BUF;
  unsigned long u0 = (u1 + PT_BUF - 1) % PT_BUF;
  unsigned long u2 = (u1 + 1) % PT_BUF;
  unsigned long u3 = (u1 + 2) % PT_BUF;
  float x0 = b[u0], x1 = b[u1], x2 = b[u2], x3 = b[u3];
  float c1 = 0.5f * (x2 - x0);
  float c2 = x0 - 2.5f * x1 + 2.0f * x2 - 0.5f * x3;
  float c3 = 0.5f * (x3 - x0) + 1.5f * (x1 - x2);
  return ((c3 * frac + c2) * frac + c1) * frac + x1;
}

static inline float pt_hann(float x) {
  if (x < 0.0f) x = 0.0f;
  if (x > 1.0f) x = 1.0f;
  return 0.5f - 0.5f * cosf((float)(2.0 * M_PI) * x);
}

static LADSPA_Handle pt_inst(const LADSPA_Descriptor *d, unsigned long sr) {
  (void)d;
  Pitch *s = calloc(1, sizeof(Pitch));
  if (s) {
    s->sr = (float)(sr ? sr : 48000);
    s->ratio_sm = 1.0f;
    s->delay = 2048.0;
  }
  return s;
}
static void pt_connect(LADSPA_Handle h, unsigned long port, LADSPA_Data *data) {
  Pitch *s = h;
  if (port < PT_N) { s->p[port] = data; s->o[port] = data; }
}
static void pt_run(LADSPA_Handle h, unsigned long n) {
  Pitch *s = h;
  float semi = s->p[PT_SEMITONES] ? clampf(*s->p[PT_SEMITONES], -12.0f, 12.0f) : 0.0f;
  float cents = s->p[PT_CENTS] ? clampf(*s->p[PT_CENTS], -100.0f, 100.0f) : 0.0f;
  float smooth = s->p[PT_SMOOTH] ? clampf(*s->p[PT_SMOOTH], 0.0f, 1.0f) : 0.65f;

  float target = powf(2.0f, (semi + cents * 0.01f) / 12.0f);
  target = clampf(target, 0.5f, 2.0f);

  float grain_ms = 20.0f + smooth * 70.0f;
  double grain = (double)(s->sr * grain_ms * 0.001f);
  if (grain < 512.0) grain = 512.0;
  if (grain > 8192.0) grain = 8192.0;
  const double min_d = 64.0;
  const double max_d = min_d + grain;

  float lerp = 0.0015f + (1.0f - smooth) * 0.025f;
  const float *il = s->p[PT_IN_L], *ir = s->p[PT_IN_R];
  float *ol = s->o[PT_OUT_L], *or_ = s->o[PT_OUT_R];
  if (!il || !ol) return;
  if (!ir) ir = il;
  if (!or_) or_ = ol;
  if (s->p[PT_BYPASS] && *s->p[PT_BYPASS] >= 0.5f) {
    for (unsigned long i = 0; i < n; i++) { ol[i] = il[i]; or_[i] = ir[i]; }
    return;
  }

  for (unsigned long i = 0; i < n; i++) {
    s->ratio_sm += (target - s->ratio_sm) * lerp;
    s->buf_l[s->w] = il[i];
    s->buf_r[s->w] = ir[i];

    if (fabsf(s->ratio_sm - 1.0f) < 0.0005f && fabsf(semi) < 0.01f && fabsf(cents) < 0.25f) {
      ol[i] = il[i];
      or_[i] = ir[i];
      s->w = (s->w + 1) % PT_BUF;
      continue;
    }

    /* delay grows when ratio < 1 (pitch down), shrinks when ratio > 1 */
    s->delay += 1.0 - (double)s->ratio_sm;
    while (s->delay >= max_d) s->delay -= grain;
    while (s->delay < min_d) s->delay += grain;

    double d0 = s->delay;
    double d1 = s->delay + grain * 0.5;
    if (d1 >= max_d) d1 -= grain;
    if (d1 < min_d) d1 += grain;

    double pos0 = (double)s->w - d0;
    double pos1 = (double)s->w - d1;
    while (pos0 < 0.0) pos0 += PT_BUF;
    while (pos1 < 0.0) pos1 += PT_BUF;

    float t0 = (float)((d0 - min_d) / grain);
    float t1 = (float)((d1 - min_d) / grain);
    float w0 = pt_hann(t0);
    float w1 = pt_hann(t1);
    float wn = w0 + w1;
    if (wn < 1.0e-6f) wn = 1.0f;

    ol[i] = (pt_cub(s->buf_l, pos0) * w0 + pt_cub(s->buf_l, pos1) * w1) / wn;
    or_[i] = (pt_cub(s->buf_r, pos0) * w0 + pt_cub(s->buf_r, pos1) * w1) / wn;

    s->w = (s->w + 1) % PT_BUF;
  }
}
static void pt_cleanup(LADSPA_Handle h) { free(h); }

/* ===================== 8-band parametric EQ ===================== */
#define EQ8_BANDS 8
enum {
  E8_IN_L = 0, E8_IN_R, E8_OUT_L, E8_OUT_R,
  /* per band: On, Freq, Gain, Q, Type  (×8) */
  E8_B0 = 4,
  E8_OUTGAIN = 4 + EQ8_BANDS * 5,
  E8_MIX,
  E8_N
};
typedef struct {
  float b0, b1, b2, a1, a2;
  float z1l, z2l, z1r, z2r;
  float last_f, last_g, last_q, last_m;
  int on;
} Eq8Band;
typedef struct {
  const LADSPA_Data *p[E8_N];
  LADSPA_Data *o[E8_N];
  Eq8Band band[EQ8_BANDS];
  float sr;
} Eq8;

static void e8_design(Eq8Band *b, float freq, float gain_db, float q, float mode, float sr) {
  float w0 = 2.0f * (float)M_PI * clampf(freq, 20.0f, sr * 0.45f) / sr;
  float cosw = cosf(w0), sinw = sinf(w0);
  float A = powf(10.0f, gain_db / 40.0f);
  q = fmaxf(q, 0.1f);
  float alpha = sinw / (2.0f * q);
  float b0, b1, b2, a0, a1, a2;
  int m = (int)mode;
  if (m == 3) {
    b0 = (1 + cosw) * 0.5f; b1 = -(1 + cosw); b2 = b0;
    a0 = 1 + alpha; a1 = -2 * cosw; a2 = 1 - alpha;
  } else if (m == 4) {
    b0 = (1 - cosw) * 0.5f; b1 = 1 - cosw; b2 = b0;
    a0 = 1 + alpha; a1 = -2 * cosw; a2 = 1 - alpha;
  } else if (m == 1) {
    float t = 2.0f * sqrtf(A) * alpha;
    b0 = A * ((A + 1) - (A - 1) * cosw + t);
    b1 = 2 * A * ((A - 1) - (A + 1) * cosw);
    b2 = A * ((A + 1) - (A - 1) * cosw - t);
    a0 = (A + 1) + (A - 1) * cosw + t;
    a1 = -2 * ((A - 1) + (A + 1) * cosw);
    a2 = (A + 1) + (A - 1) * cosw - t;
  } else if (m == 2) {
    float t = 2.0f * sqrtf(A) * alpha;
    b0 = A * ((A + 1) + (A - 1) * cosw + t);
    b1 = -2 * A * ((A - 1) + (A + 1) * cosw);
    b2 = A * ((A + 1) + (A - 1) * cosw - t);
    a0 = (A + 1) - (A - 1) * cosw + t;
    a1 = 2 * ((A - 1) - (A + 1) * cosw);
    a2 = (A + 1) - (A - 1) * cosw - t;
  } else {
    b0 = 1 + alpha * A; b1 = -2 * cosw; b2 = 1 - alpha * A;
    a0 = 1 + alpha / A; a1 = -2 * cosw; a2 = 1 - alpha / A;
  }
  b->b0 = b0 / a0; b->b1 = b1 / a0; b->b2 = b2 / a0;
  b->a1 = a1 / a0; b->a2 = a2 / a0;
  b->last_f = freq; b->last_g = gain_db; b->last_q = q; b->last_m = mode;
}

static LADSPA_Handle e8_inst(const LADSPA_Descriptor *d, unsigned long sr) {
  (void)d;
  Eq8 *s = calloc(1, sizeof(Eq8));
  static const float def_f[EQ8_BANDS] = {60,150,400,1000,2500,5000,8000,12000};
  if (s) {
    s->sr = (float)sr;
    for (int i = 0; i < EQ8_BANDS; i++) {
      s->band[i].on = 1;
      e8_design(&s->band[i], def_f[i], 0.0f, 0.707f, 0.0f, s->sr);
    }
  }
  return s;
}
static void e8_connect(LADSPA_Handle h, unsigned long port, LADSPA_Data *data) {
  Eq8 *s = h;
  if (port < E8_N) { s->p[port] = data; s->o[port] = data; }
}
static void e8_run(LADSPA_Handle h, unsigned long n) {
  Eq8 *s = h;
  float out_g = s->p[E8_OUTGAIN] ? db_to_lin(*s->p[E8_OUTGAIN]) : 1.0f;
  float mix = s->p[E8_MIX] ? clampf(*s->p[E8_MIX], 0.0f, 1.0f) : 1.0f;
  const float *il = s->p[E8_IN_L], *ir = s->p[E8_IN_R];
  float *ol = s->o[E8_OUT_L], *or_ = s->o[E8_OUT_R];
  if (!il || !ol) return;
  if (!ir) ir = il;
  if (!or_) or_ = ol;
  if (mix < 1.0e-5f) {
    for (unsigned long i = 0; i < n; i++) { ol[i] = il[i]; or_[i] = ir[i]; }
    return;
  }
  for (int bi = 0; bi < EQ8_BANDS; bi++) {
    int base = E8_B0 + bi * 5;
    float on = s->p[base] ? *s->p[base] : 1.0f;
    float freq = s->p[base+1] ? *s->p[base+1] : 1000.0f;
    float gain = s->p[base+2] ? *s->p[base+2] : 0.0f;
    float q = s->p[base+3] ? *s->p[base+3] : 0.707f;
    float mode = s->p[base+4] ? *s->p[base+4] : 0.0f;
    s->band[bi].on = on >= 0.5f;
    Eq8Band *b = &s->band[bi];
    if (freq != b->last_f || gain != b->last_g || q != b->last_q || mode != b->last_m)
      e8_design(b, freq, gain, q, mode, s->sr);
  }
  for (unsigned long i = 0; i < n; i++) {
    float l = il[i], r = ir[i];
    for (int bi = 0; bi < EQ8_BANDS; bi++) {
      Eq8Band *b = &s->band[bi];
      if (!b->on) continue;
      float y = b->b0 * l + b->z1l;
      b->z1l = b->b1 * l - b->a1 * y + b->z2l;
      b->z2l = b->b2 * l - b->a2 * y;
      l = y;
      y = b->b0 * r + b->z1r;
      b->z1r = b->b1 * r - b->a1 * y + b->z2r;
      b->z2r = b->b2 * r - b->a2 * y;
      r = y;
    }
    float wl = l * out_g;
    float wr = r * out_g;
    ol[i] = il[i] + mix * (wl - il[i]);
    or_[i] = ir[i] + mix * (wr - ir[i]);
  }
}
static void e8_cleanup(LADSPA_Handle h) { free(h); }

/* ===================== Theatre Drive (Blood Overdrive–class, cinema defaults) =====================
 * Flow: split → pre LP/BP blend → drive (+bias, character) → 2-pole post LP → post gain → mix.
 * Focus: Full | Drive Bass | Protect Bass — saturate through separation.
 * 2× oversampling on the nonlinearity + DC block for clean harmonics. */
enum {
  OD_IN_L = 0, OD_IN_R, OD_OUT_L, OD_OUT_R,
  OD_PREBAND, OD_COLOR, OD_PRESHAPE, OD_DRIVE, OD_BOOST,
  OD_CHAR, OD_BIAS, OD_POSTFILT, OD_POSTGAIN, OD_MIX,
  OD_SPLIT, OD_FOCUS, OD_N
};

typedef struct {
  float b0, b1, b2, a1, a2;
  float z1l, z2l, z1r, z2r;
} OdBq;

typedef struct {
  const LADSPA_Data *p[OD_N];
  LADSPA_Data *o[OD_N];
  float sr;
  OdBq pre;              /* pre LP/BP coeffs + stereo z */
  OdBq post;             /* 2-pole post LP */
  float xl_lp, xr_lp;    /* one-pole split */
  float dc_x_l, dc_y_l, dc_x_r, dc_y_r;
  float prev_l, prev_r;
  float last_color, last_postf, last_shape;
} Overdrive;

static void od_bq_lp(OdBq *f, float freq, float q, float sr) {
  float w0 = 2.0f * (float)M_PI * clampf(freq, 20.0f, sr * 0.45f) / sr;
  float cosw = cosf(w0), sinw = sinf(w0);
  float alpha = sinw / (2.0f * fmaxf(q, 0.3f));
  float b0 = (1.0f - cosw) * 0.5f, b1 = 1.0f - cosw, b2 = b0;
  float a0 = 1.0f + alpha, a1 = -2.0f * cosw, a2 = 1.0f - alpha;
  f->b0 = b0 / a0; f->b1 = b1 / a0; f->b2 = b2 / a0;
  f->a1 = a1 / a0; f->a2 = a2 / a0;
}

static void od_bq_bp(OdBq *f, float freq, float q, float sr) {
  float w0 = 2.0f * (float)M_PI * clampf(freq, 20.0f, sr * 0.45f) / sr;
  float cosw = cosf(w0), sinw = sinf(w0);
  float alpha = sinw / (2.0f * fmaxf(q, 0.3f));
  float b0 = alpha, b1 = 0.0f, b2 = -alpha;
  float a0 = 1.0f + alpha, a1 = -2.0f * cosw, a2 = 1.0f - alpha;
  f->b0 = b0 / a0; f->b1 = b1 / a0; f->b2 = b2 / a0;
  f->a1 = a1 / a0; f->a2 = a2 / a0;
}

static inline float od_bq_tick(OdBq *f, float x, int ch) {
  float *z1 = ch ? &f->z1r : &f->z1l;
  float *z2 = ch ? &f->z2r : &f->z2l;
  float y = f->b0 * x + *z1;
  *z1 = f->b1 * x - f->a1 * y + *z2;
  *z2 = f->b2 * x - f->a2 * y;
  return y;
}

static inline float od_shape(float x, int character) {
  switch (character) {
  case 1: /* Soft */
    return tanhf(x * 0.85f) * 1.08f;
  case 2: { /* Hard */
    float a = fabsf(x);
    float y = a < 1.0f ? a - a * a * a / 3.0f : 0.6666667f;
    return copysignf(y * 1.15f, x);
  }
  case 3: { /* Diode */
    float pos = x >= 0.0f ? (1.0f - expf(-x * 1.4f)) : 0.0f;
    float neg = x < 0.0f ? -(0.65f * (1.0f - expf(x * 1.8f))) : 0.0f;
    return (pos + neg) * 1.2f;
  }
  default: { /* Tube */
    float y = tanhf(x);
    return y + 0.04f * y * y * y;
  }
  }
}

static inline float od_dcb(float x, float *x1, float *y1, float R) {
  float y = x - *x1 + R * (*y1);
  *x1 = x;
  *y1 = y;
  return y;
}

static inline float od_drive_sample(float x, float gain, float bias, int character) {
  return od_shape(x * gain + bias, character);
}

static LADSPA_Handle od_inst(const LADSPA_Descriptor *d, unsigned long sr) {
  (void)d;
  Overdrive *s = calloc(1, sizeof(Overdrive));
  if (s) {
    s->sr = (float)(sr ? sr : 48000);
    od_bq_lp(&s->pre, 180.0f, 0.707f, s->sr);
    od_bq_lp(&s->post, 3200.0f, 0.707f, s->sr);
    s->last_color = 180.0f;
    s->last_postf = 3200.0f;
    s->last_shape = 0.0f;
  }
  return s;
}
static void od_connect(LADSPA_Handle h, unsigned long port, LADSPA_Data *data) {
  Overdrive *s = h;
  if (port < OD_N) { s->p[port] = data; s->o[port] = data; }
}
static void od_cleanup(LADSPA_Handle h) { free(h); }

static void od_run(LADSPA_Handle h, unsigned long n) {
  Overdrive *s = h;
  float preband = s->p[OD_PREBAND] ? clampf(*s->p[OD_PREBAND], 0.0f, 1.0f) : 0.55f;
  float color = s->p[OD_COLOR] ? clampf(*s->p[OD_COLOR], 40.0f, 8000.0f) : 180.0f;
  float preshape = s->p[OD_PRESHAPE] ? clampf(*s->p[OD_PRESHAPE], 0.0f, 1.0f) : 0.0f;
  float drive = s->p[OD_DRIVE] ? clampf(*s->p[OD_DRIVE], 0.0f, 1.0f) : 0.34f;
  int boost = s->p[OD_BOOST] && *s->p[OD_BOOST] >= 0.5f;
  int character = s->p[OD_CHAR] ? (int)clampf(*s->p[OD_CHAR], 0.0f, 3.0f) : 0;
  float bias = s->p[OD_BIAS] ? clampf(*s->p[OD_BIAS], -1.0f, 1.0f) : 0.10f;
  float postf = s->p[OD_POSTFILT] ? clampf(*s->p[OD_POSTFILT], 200.0f, 20000.0f) : 3200.0f;
  float postg = s->p[OD_POSTGAIN] ? clampf(*s->p[OD_POSTGAIN], 0.0f, 1.0f) : 0.42f;
  float mix = s->p[OD_MIX] ? clampf(*s->p[OD_MIX], 0.0f, 1.0f) : 0.68f;
  float split = s->p[OD_SPLIT] ? clampf(*s->p[OD_SPLIT], 20.0f, 500.0f) : 100.0f;
  int focus = s->p[OD_FOCUS] ? (int)clampf(*s->p[OD_FOCUS], 0.0f, 2.0f) : 1;

  const float *il = s->p[OD_IN_L], *ir = s->p[OD_IN_R];
  float *ol = s->o[OD_OUT_L], *or_ = s->o[OD_OUT_R];
  if (!il || !ol) return;
  if (!ir) ir = il;
  if (!or_) or_ = ol;
  if (mix < 1.0e-5f) {
    for (unsigned long i = 0; i < n; i++) { ol[i] = il[i]; or_[i] = ir[i]; }
    return;
  }

  if (color != s->last_color || preshape != s->last_shape) {
    if (preshape >= 0.5f)
      od_bq_bp(&s->pre, color, 0.85f, s->sr);
    else
      od_bq_lp(&s->pre, color, 0.707f, s->sr);
    s->last_color = color;
    s->last_shape = preshape;
  }
  if (postf != s->last_postf) {
    od_bq_lp(&s->post, postf, 0.707f, s->sr);
    s->last_postf = postf;
  }

  float gain = 1.0f + drive * drive * 36.0f;
  if (boost) gain *= 10.0f;
  float bias_amt = bias * 0.22f;
  float makeup = postg * (1.15f / (0.35f + drive * 0.9f + (boost ? 0.8f : 0.0f)));
  float split_c = expf(-2.0f * (float)M_PI * split / s->sr);
  const float dcR = 0.995f;

  for (unsigned long i = 0; i < n; i++) {
    float in_l = il[i], in_r = ir[i];

    s->xl_lp = split_c * s->xl_lp + (1.0f - split_c) * in_l;
    s->xr_lp = split_c * s->xr_lp + (1.0f - split_c) * in_r;
    float lo_l = s->xl_lp, lo_r = s->xr_lp;
    float hi_l = in_l - lo_l, hi_r = in_r - lo_r;

    float src_l, src_r, pass_l, pass_r, dry_l, dry_r;
    if (focus == 1) { /* Drive Bass — saturate lows, keep highs dry */
      src_l = lo_l; src_r = lo_r;
      pass_l = hi_l; pass_r = hi_r;
      dry_l = lo_l; dry_r = lo_r;
    } else if (focus == 2) { /* Protect Bass — saturate highs, keep lows dry */
      src_l = hi_l; src_r = hi_r;
      pass_l = lo_l; pass_r = lo_r;
      dry_l = hi_l; dry_r = hi_r;
    } else {
      src_l = in_l; src_r = in_r;
      pass_l = pass_r = 0.0f;
      dry_l = in_l; dry_r = in_r;
    }

    float fl = od_bq_tick(&s->pre, src_l, 0);
    float fr = od_bq_tick(&s->pre, src_r, 1);
    float pl = src_l + preband * (fl - src_l);
    float pr = src_r + preband * (fr - src_r);

    float mid_l = 0.5f * (s->prev_l + pl);
    float mid_r = 0.5f * (s->prev_r + pr);
    float wl = 0.5f * (od_drive_sample(mid_l, gain, bias_amt, character)
                     + od_drive_sample(pl, gain, bias_amt, character));
    float wr = 0.5f * (od_drive_sample(mid_r, gain, bias_amt, character)
                     + od_drive_sample(pr, gain, bias_amt, character));
    s->prev_l = pl;
    s->prev_r = pr;

    wl = od_dcb(wl, &s->dc_x_l, &s->dc_y_l, dcR);
    wr = od_dcb(wr, &s->dc_x_r, &s->dc_y_r, dcR);

    wl = od_bq_tick(&s->post, wl, 0) * makeup;
    wr = od_bq_tick(&s->post, wr, 1) * makeup;

    float wet_l = dry_l + mix * (wl - dry_l);
    float wet_r = dry_r + mix * (wr - dry_r);
    ol[i] = wet_l + pass_l;
    or_[i] = wet_r + pass_r;
  }
}

/* ---- descriptor table ---- */
static LADSPA_Descriptor D[N_PLUGINS];
static int ready = 0;

static LADSPA_PortDescriptor pd_audio_io[4];
static void fill_audio_ports(LADSPA_PortDescriptor *pd, unsigned n_ctrl_start, unsigned n_total,
                             LADSPA_PortDescriptor *all) {
  (void)n_ctrl_start;
  for (unsigned i = 0; i < 4; i++)
    all[i] = (i < 2) ? (LADSPA_PORT_INPUT | LADSPA_PORT_AUDIO)
                     : (LADSPA_PORT_OUTPUT | LADSPA_PORT_AUDIO);
  for (unsigned i = 4; i < n_total; i++)
    all[i] = LADSPA_PORT_INPUT | LADSPA_PORT_CONTROL;
  (void)pd;
}

#define MAXPORTS 48
static LADSPA_PortDescriptor PD[N_PLUGINS][MAXPORTS];
static LADSPA_PortRangeHint PH[N_PLUGINS][MAXPORTS];
static const char *PN[N_PLUGINS][MAXPORTS];

static void init_all(void) {
  if (ready) return;
  /* Softclip 392010 — Fruity Soft Clipper–style (Threshold + Post + Mix) */
  PN[0][0]="Input L"; PN[0][1]="Input R"; PN[0][2]="Output L"; PN[0][3]="Output R";
  PN[0][4]="Threshold"; PN[0][5]="Post"; PN[0][6]="Mix";
  for (int i=0;i<4;i++) PD[0][i]=(i<2)?(LADSPA_PORT_INPUT|LADSPA_PORT_AUDIO):(LADSPA_PORT_OUTPUT|LADSPA_PORT_AUDIO);
  PD[0][4]=PD[0][5]=PD[0][6]=LADSPA_PORT_INPUT|LADSPA_PORT_CONTROL;
  memset(PH[0],0,sizeof(PH[0]));
  PH[0][4].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_MIDDLE;
  PH[0][4].LowerBound=0.05f; PH[0][4].UpperBound=1.0f;
  PH[0][5].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_1;
  PH[0][5].LowerBound=0.0f; PH[0][5].UpperBound=4.0f;
  PH[0][6].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_1;
  PH[0][6].LowerBound=0.0f; PH[0][6].UpperBound=1.0f;
  D[0].UniqueID=392010; D[0].Label="buschain_softclip"; D[0].Name="BusChain Soft Clipper";
  D[0].Maker="BusChain Control"; D[0].Copyright="MIT"; D[0].PortCount=SC_N;
  D[0].PortDescriptors=PD[0]; D[0].PortNames=PN[0]; D[0].PortRangeHints=PH[0];
  D[0].Properties=LADSPA_PROPERTY_HARD_RT_CAPABLE;
  D[0].instantiate=sc_inst; D[0].connect_port=sc_connect; D[0].run=sc_run; D[0].cleanup=sc_cleanup;

  /* Limiter 392011 */
  PN[1][0]="Input L"; PN[1][1]="Input R"; PN[1][2]="Output L"; PN[1][3]="Output R";
  PN[1][4]="Ceiling (dB)"; PN[1][5]="Release (ms)"; PN[1][6]="Bypass";
  for (int i=0;i<4;i++) PD[1][i]=(i<2)?(LADSPA_PORT_INPUT|LADSPA_PORT_AUDIO):(LADSPA_PORT_OUTPUT|LADSPA_PORT_AUDIO);
  for (int i=4;i<LM_N;i++) PD[1][i]=LADSPA_PORT_INPUT|LADSPA_PORT_CONTROL;
  memset(PH[1],0,sizeof(PH[1]));
  PH[1][4].LowerBound=-24; PH[1][4].UpperBound=0;
  PH[1][4].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_HIGH;
  PH[1][5].LowerBound=1; PH[1][5].UpperBound=500;
  PH[1][5].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_MIDDLE;
  PH[1][6].LowerBound=0; PH[1][6].UpperBound=1;
  PH[1][6].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_TOGGLED|LADSPA_HINT_DEFAULT_0;
  D[1].UniqueID=392011; D[1].Label="buschain_limiter"; D[1].Name="BusChain Limiter";
  D[1].Maker="BusChain Control"; D[1].Copyright="MIT"; D[1].PortCount=LM_N;
  D[1].PortDescriptors=PD[1]; D[1].PortNames=PN[1]; D[1].PortRangeHints=PH[1];
  D[1].Properties=LADSPA_PROPERTY_HARD_RT_CAPABLE;
  D[1].instantiate=lm_inst; D[1].connect_port=lm_connect; D[1].run=lm_run; D[1].cleanup=lm_cleanup;

  /* Compressor 392012 */
  PN[2][0]="Input L"; PN[2][1]="Input R"; PN[2][2]="Output L"; PN[2][3]="Output R";
  PN[2][4]="Threshold (dB)"; PN[2][5]="Ratio"; PN[2][6]="Attack (ms)"; PN[2][7]="Release (ms)"; PN[2][8]="Makeup (dB)";
  PN[2][9]="Bypass";
  for (int i=0;i<4;i++) PD[2][i]=(i<2)?(LADSPA_PORT_INPUT|LADSPA_PORT_AUDIO):(LADSPA_PORT_OUTPUT|LADSPA_PORT_AUDIO);
  for (int i=4;i<CM_N;i++) PD[2][i]=LADSPA_PORT_INPUT|LADSPA_PORT_CONTROL;
  memset(PH[2],0,sizeof(PH[2]));
  PH[2][4].LowerBound=-60; PH[2][4].UpperBound=0;
  PH[2][5].LowerBound=1; PH[2][5].UpperBound=20;
  PH[2][6].LowerBound=0.1f; PH[2][6].UpperBound=100;
  PH[2][7].LowerBound=1; PH[2][7].UpperBound=1000;
  PH[2][8].LowerBound=-24; PH[2][8].UpperBound=24;
  for (int i=4;i<CM_N-1;i++) PH[2][i].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_MIDDLE;
  PH[2][9].LowerBound=0; PH[2][9].UpperBound=1;
  PH[2][9].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_TOGGLED|LADSPA_HINT_DEFAULT_0;
  D[2].UniqueID=392012; D[2].Label="buschain_compressor"; D[2].Name="BusChain Compressor";
  D[2].Maker="BusChain Control"; D[2].Copyright="MIT"; D[2].PortCount=CM_N;
  D[2].PortDescriptors=PD[2]; D[2].PortNames=PN[2]; D[2].PortRangeHints=PH[2];
  D[2].Properties=LADSPA_PROPERTY_HARD_RT_CAPABLE;
  D[2].instantiate=cm_inst; D[2].connect_port=cm_connect; D[2].run=cm_run; D[2].cleanup=cm_cleanup;

  /* EQ 392013 */
  PN[3][0]="Input L"; PN[3][1]="Input R"; PN[3][2]="Output L"; PN[3][3]="Output R";
  PN[3][4]="Freq (Hz)"; PN[3][5]="Gain (dB)"; PN[3][6]="Q"; PN[3][7]="Mode"; PN[3][8]="Bypass";
  for (int i=0;i<4;i++) PD[3][i]=(i<2)?(LADSPA_PORT_INPUT|LADSPA_PORT_AUDIO):(LADSPA_PORT_OUTPUT|LADSPA_PORT_AUDIO);
  for (int i=4;i<EQ_N;i++) PD[3][i]=LADSPA_PORT_INPUT|LADSPA_PORT_CONTROL;
  memset(PH[3],0,sizeof(PH[3]));
  PH[3][4].LowerBound=20; PH[3][4].UpperBound=20000;
  PH[3][4].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_LOGARITHMIC|LADSPA_HINT_DEFAULT_MIDDLE;
  PH[3][5].LowerBound=-24; PH[3][5].UpperBound=24;
  PH[3][6].LowerBound=0.1f; PH[3][6].UpperBound=10;
  PH[3][7].LowerBound=0; PH[3][7].UpperBound=4; /* 0 peak 1 LS 2 HS 3 HP 4 LP */
  for (int i=5;i<EQ_N-1;i++) PH[3][i].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_MIDDLE;
  PH[3][8].LowerBound=0; PH[3][8].UpperBound=1;
  PH[3][8].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_TOGGLED|LADSPA_HINT_DEFAULT_0;
  D[3].UniqueID=392013; D[3].Label="buschain_eq"; D[3].Name="BusChain EQ";
  D[3].Maker="BusChain Control"; D[3].Copyright="MIT"; D[3].PortCount=EQ_N;
  D[3].PortDescriptors=PD[3]; D[3].PortNames=PN[3]; D[3].PortRangeHints=PH[3];
  D[3].Properties=LADSPA_PROPERTY_HARD_RT_CAPABLE;
  D[3].instantiate=eq_inst; D[3].connect_port=eq_connect; D[3].run=eq_run; D[3].cleanup=eq_cleanup;

  /* Pitch 392014 — dual-grain crossfade */
  PN[4][0]="Input L"; PN[4][1]="Input R"; PN[4][2]="Output L"; PN[4][3]="Output R";
  PN[4][4]="Semitones"; PN[4][5]="Cents"; PN[4][6]="Smooth"; PN[4][7]="Bypass";
  for (int i=0;i<4;i++) PD[4][i]=(i<2)?(LADSPA_PORT_INPUT|LADSPA_PORT_AUDIO):(LADSPA_PORT_OUTPUT|LADSPA_PORT_AUDIO);
  for (int i=4;i<PT_N;i++) PD[4][i]=LADSPA_PORT_INPUT|LADSPA_PORT_CONTROL;
  memset(PH[4],0,sizeof(PH[4]));
  PH[4][4].LowerBound=-12; PH[4][4].UpperBound=12;
  PH[4][4].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_0;
  PH[4][5].LowerBound=-100; PH[4][5].UpperBound=100;
  PH[4][5].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_0;
  PH[4][6].LowerBound=0; PH[4][6].UpperBound=1;
  PH[4][6].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_MIDDLE;
  PH[4][7].LowerBound=0; PH[4][7].UpperBound=1;
  PH[4][7].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_TOGGLED|LADSPA_HINT_DEFAULT_0;
  D[4].UniqueID=392014; D[4].Label="buschain_pitch"; D[4].Name="BusChain Pitch";
  D[4].Maker="BusChain Control"; D[4].Copyright="MIT"; D[4].PortCount=PT_N;
  D[4].PortDescriptors=PD[4]; D[4].PortNames=PN[4]; D[4].PortRangeHints=PH[4];
  D[4].Properties=LADSPA_PROPERTY_HARD_RT_CAPABLE;
  D[4].instantiate=pt_inst; D[4].connect_port=pt_connect; D[4].run=pt_run; D[4].cleanup=pt_cleanup;

  /* EQ8 392015 */
  {
    static const float def_f[EQ8_BANDS] = {60,150,400,1000,2500,5000,8000,12000};
    static char names[E8_N][24];
    PN[5][0]="Input L"; PN[5][1]="Input R"; PN[5][2]="Output L"; PN[5][3]="Output R";
    for (int i=0;i<4;i++) PD[5][i]=(i<2)?(LADSPA_PORT_INPUT|LADSPA_PORT_AUDIO):(LADSPA_PORT_OUTPUT|LADSPA_PORT_AUDIO);
    for (int bi=0; bi<EQ8_BANDS; bi++) {
      int base = E8_B0 + bi*5;
      snprintf(names[base+0], 24, "B%d On", bi+1);
      snprintf(names[base+1], 24, "B%d Freq", bi+1);
      snprintf(names[base+2], 24, "B%d Gain", bi+1);
      snprintf(names[base+3], 24, "B%d Q", bi+1);
      snprintf(names[base+4], 24, "B%d Type", bi+1);
      PN[5][base+0]=names[base+0];
      PN[5][base+1]=names[base+1];
      PN[5][base+2]=names[base+2];
      PN[5][base+3]=names[base+3];
      PN[5][base+4]=names[base+4];
      for (int k=0;k<5;k++) PD[5][base+k]=LADSPA_PORT_INPUT|LADSPA_PORT_CONTROL;
      memset(&PH[5][base], 0, sizeof(PH[5][base])*5);
      PH[5][base+0].LowerBound=0; PH[5][base+0].UpperBound=1;
      PH[5][base+0].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_TOGGLED|LADSPA_HINT_DEFAULT_1;
      PH[5][base+1].LowerBound=20; PH[5][base+1].UpperBound=20000;
      PH[5][base+1].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_LOGARITHMIC|LADSPA_HINT_DEFAULT_MIDDLE;
      PH[5][base+2].LowerBound=-24; PH[5][base+2].UpperBound=24;
      PH[5][base+3].LowerBound=0.1f; PH[5][base+3].UpperBound=10;
      PH[5][base+4].LowerBound=0; PH[5][base+4].UpperBound=4;
      for (int k=2;k<5;k++) PH[5][base+k].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_MIDDLE;
      (void)def_f;
    }
    PN[5][E8_OUTGAIN]="Output (dB)";
    PD[5][E8_OUTGAIN]=LADSPA_PORT_INPUT|LADSPA_PORT_CONTROL;
    PH[5][E8_OUTGAIN].LowerBound=-24; PH[5][E8_OUTGAIN].UpperBound=24;
    PH[5][E8_OUTGAIN].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_0;
    PN[5][E8_MIX]="Mix";
    PD[5][E8_MIX]=LADSPA_PORT_INPUT|LADSPA_PORT_CONTROL;
    PH[5][E8_MIX].LowerBound=0; PH[5][E8_MIX].UpperBound=1;
    PH[5][E8_MIX].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_1;
    D[5].UniqueID=392015; D[5].Label="buschain_eq8"; D[5].Name="BusChain EQ 8-Band";
    D[5].Maker="BusChain Control"; D[5].Copyright="MIT"; D[5].PortCount=E8_N;
    D[5].PortDescriptors=PD[5]; D[5].PortNames=PN[5]; D[5].PortRangeHints=PH[5];
    D[5].Properties=LADSPA_PROPERTY_HARD_RT_CAPABLE;
    D[5].instantiate=e8_inst; D[5].connect_port=e8_connect; D[5].run=e8_run; D[5].cleanup=e8_cleanup;
  }

  /* Theatre Drive 392016 — Blood Overdrive–class with bass split */
  PN[6][0]="Input L"; PN[6][1]="Input R"; PN[6][2]="Output L"; PN[6][3]="Output R";
  PN[6][4]="Pre Band"; PN[6][5]="Color (Hz)"; PN[6][6]="Pre Shape";
  PN[6][7]="Drive"; PN[6][8]="Boost"; PN[6][9]="Character"; PN[6][10]="Bias";
  PN[6][11]="Post Filter (Hz)"; PN[6][12]="Post Gain"; PN[6][13]="Mix";
  PN[6][14]="Split (Hz)"; PN[6][15]="Focus";
  for (int i=0;i<4;i++) PD[6][i]=(i<2)?(LADSPA_PORT_INPUT|LADSPA_PORT_AUDIO):(LADSPA_PORT_OUTPUT|LADSPA_PORT_AUDIO);
  for (int i=4;i<OD_N;i++) PD[6][i]=LADSPA_PORT_INPUT|LADSPA_PORT_CONTROL;
  memset(PH[6],0,sizeof(PH[6]));
  /* Cinema defaults via HINT_DEFAULT_* where possible; catalog carries exact defaults */
  PH[6][4].LowerBound=0; PH[6][4].UpperBound=1;
  PH[6][4].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_MIDDLE;
  PH[6][5].LowerBound=40; PH[6][5].UpperBound=8000;
  PH[6][5].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_LOGARITHMIC|LADSPA_HINT_DEFAULT_LOW;
  PH[6][6].LowerBound=0; PH[6][6].UpperBound=1;
  PH[6][6].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_INTEGER|LADSPA_HINT_DEFAULT_0;
  PH[6][7].LowerBound=0; PH[6][7].UpperBound=1;
  PH[6][7].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_LOW;
  PH[6][8].LowerBound=0; PH[6][8].UpperBound=1;
  PH[6][8].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_TOGGLED|LADSPA_HINT_DEFAULT_0;
  PH[6][9].LowerBound=0; PH[6][9].UpperBound=3;
  PH[6][9].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_INTEGER|LADSPA_HINT_DEFAULT_0;
  PH[6][10].LowerBound=-1; PH[6][10].UpperBound=1;
  PH[6][10].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_0;
  PH[6][11].LowerBound=200; PH[6][11].UpperBound=20000;
  PH[6][11].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_LOGARITHMIC|LADSPA_HINT_DEFAULT_MIDDLE;
  PH[6][12].LowerBound=0; PH[6][12].UpperBound=1;
  PH[6][12].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_MIDDLE;
  PH[6][13].LowerBound=0; PH[6][13].UpperBound=1;
  PH[6][13].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_DEFAULT_HIGH;
  PH[6][14].LowerBound=20; PH[6][14].UpperBound=500;
  PH[6][14].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_LOGARITHMIC|LADSPA_HINT_DEFAULT_MIDDLE;
  PH[6][15].LowerBound=0; PH[6][15].UpperBound=2;
  PH[6][15].HintDescriptor=LADSPA_HINT_BOUNDED_BELOW|LADSPA_HINT_BOUNDED_ABOVE|LADSPA_HINT_INTEGER|LADSPA_HINT_DEFAULT_1;
  D[6].UniqueID=392016; D[6].Label="buschain_overdrive"; D[6].Name="BusChain Theatre Drive";
  D[6].Maker="BusChain Control"; D[6].Copyright="MIT"; D[6].PortCount=OD_N;
  D[6].PortDescriptors=PD[6]; D[6].PortNames=PN[6]; D[6].PortRangeHints=PH[6];
  D[6].Properties=LADSPA_PROPERTY_HARD_RT_CAPABLE;
  D[6].instantiate=od_inst; D[6].connect_port=od_connect; D[6].run=od_run; D[6].cleanup=od_cleanup;

  ready = 1;
  (void)fill_audio_ports;
  (void)pd_audio_io;
}

const LADSPA_Descriptor *ladspa_descriptor(unsigned long index) {
  init_all();
  return index < N_PLUGINS ? &D[index] : NULL;
}
