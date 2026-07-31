/* BusChain Room — Predelay → image-source ER → 8-FDN late field */
#include "dsp.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

#define FDN_N 8
#define ER_TAPS 12
#define MAX_SR 96000
#define MAX_PREDELAY ((MAX_SR * 200) / 1000 + 4)
#define MAX_LINE ((MAX_SR * 3) / 2) /* 1.5 s */

static float clampf(float x, float lo, float hi) {
  return x < lo ? lo : (x > hi ? hi : x);
}
static inline int f_ok(float x) { return x == x && fabsf(x) < 1.0e20f; }
static inline float ms_to_coeff(float ms, float sr) {
  if (ms <= 0.0f || sr < 1.0f) return 0.0f;
  return expf(-1.0f / (0.001f * ms * sr));
}
static inline void smooth_toward(float *cur, float tgt, float c) {
  *cur = c * (*cur) + (1.0f - c) * tgt;
}

/* Prime-ish base delays (samples @ 48k), scaled by size/sr */
static const int BASE_DELAYS[FDN_N] = {
  1601, 1867, 2053, 2251, 2447, 2677, 2903, 3121
};

typedef struct {
  float *buf;
  int size;
  int w;
} DelayLine;

static void dl_init(DelayLine *d, int size) {
  d->size = size > 4 ? size : 4;
  d->buf = (float *)calloc((size_t)d->size, sizeof(float));
  d->w = 0;
}
static void dl_free(DelayLine *d) {
  free(d->buf);
  d->buf = NULL;
}
static void dl_clear(DelayLine *d) {
  if (d->buf) memset(d->buf, 0, (size_t)d->size * sizeof(float));
  d->w = 0;
}
static inline void dl_write(DelayLine *d, float x) {
  d->buf[d->w] = x;
  d->w++;
  if (d->w >= d->size) d->w = 0;
}
static inline float dl_read(const DelayLine *d, float delay) {
  float dly = clampf(delay, 1.0f, (float)(d->size - 2));
  int di = (int)dly;
  float frac = dly - (float)di;
  int i0 = d->w - di;
  while (i0 < 0) i0 += d->size;
  int i1 = i0 - 1;
  if (i1 < 0) i1 += d->size;
  return d->buf[i0] + frac * (d->buf[i1] - d->buf[i0]);
}

/* One-pole: y[n] = b0*x[n] + b1*x[n-1] + a1*y[n-1] */
typedef struct {
  float b0, b1, a1;
  float xz, yz;
} Filter1;

static void f1_lp(Filter1 *f, float cutoff, float sr) {
  float c = expf(-2.0f * (float)M_PI * clampf(cutoff, 40.0f, sr * 0.45f) / sr);
  f->b0 = 1.0f - c;
  f->b1 = 0.0f;
  f->a1 = c;
}
static void f1_hp(Filter1 *f, float cutoff, float sr) {
  float c = expf(-2.0f * (float)M_PI * clampf(cutoff, 20.0f, sr * 0.45f) / sr);
  f->b0 = (1.0f + c) * 0.5f;
  f->b1 = -(1.0f + c) * 0.5f;
  f->a1 = c;
}
static inline float f1_tick(Filter1 *f, float x) {
  float y = f->b0 * x + f->b1 * f->xz + f->a1 * f->yz;
  f->xz = x;
  f->yz = y;
  if (!f_ok(y)) {
    f->xz = f->yz = 0.0f;
    return 0.0f;
  }
  if (fabsf(f->yz) < 1.0e-20f) f->yz = 0.0f;
  return y;
}

struct BuschainReverbState {
  double sr;
  BuschainReverbParams p;
  BuschainReverbMeters meters;

  DelayLine predelay_l;
  DelayLine predelay_r;
  DelayLine er[ER_TAPS];
  float er_gains[ER_TAPS];
  float er_delays[ER_TAPS];
  int er_in_ch[ER_TAPS];  /* 0 = left speaker/input, 1 = right */
  int er_out_ch[ER_TAPS]; /* 0 = left ear → wet L, 1 = right ear → wet R */

  DelayLine fdn[FDN_N];
  float fdn_len[FDN_N];
  Filter1 damp[FDN_N];
  float lfo_phase[FDN_N];

  Filter1 wet_hp_l, wet_hp_r, wet_lp_l, wet_lp_r;
  float duck_env;
  float gate_env;
  float sm_mix, sm_size, sm_rt60;
  float stereo_mix; /* 0 mono … 1 full stereo spacing */

  float energy_env;
  float er_energy;
  float tail_energy;
  uint32_t sample_count;
};

void buschain_reverb_default_params(BuschainReverbParams *p) {
  p->bypass = 0;
  p->mix = 0.25f;
  p->predelay_ms = 20.0f;
  p->size = 1.0f;
  p->shape = 1.0f;
  p->rt60 = 1.8f;
  p->character = 0.55f;
  p->er_level = 0.55f;
  p->er_spread = 0.5f;
  p->diffusion = 0.65f;
  p->density = 0.7f;
  p->modulation = 0.15f;
  p->decay_lo = 1.0f;
  p->decay_hi = 0.7f;
  p->wet_hp_hz = 80.0f;
  p->wet_lp_hz = 12000.0f;
  p->width = 0.85f;
  p->duck_amount = 0.0f;
  p->duck_release_ms = 200.0f;
  p->freeze = 0.0f;
  p->gate_time_ms = 0.0f;
  p->room_type = 0.0f;
  p->source_x = 0.28f;
  p->source_y = 0.55f;
  p->source_z = 0.30f;
  p->listener_x = 0.72f;
  p->listener_y = 0.50f;
  p->listener_z = 0.70f;
  p->source_spacing = 0.35f;
  p->source_yaw = 0.0f;
  p->face_lock = 1.0f;
  p->listener_spacing = 0.35f;
  p->ear_angle = 180.0f;
  p->ear_preset = 0.0f;
}

BuschainReverbState *buschain_reverb_create(void) {
  BuschainReverbState *s = (BuschainReverbState *)calloc(1, sizeof(*s));
  if (!s) return NULL;
  buschain_reverb_default_params(&s->p);
  dl_init(&s->predelay_l, MAX_PREDELAY);
  dl_init(&s->predelay_r, MAX_PREDELAY);
  for (int i = 0; i < ER_TAPS; i++) dl_init(&s->er[i], MAX_PREDELAY + 8192);
  for (int i = 0; i < FDN_N; i++) dl_init(&s->fdn[i], MAX_LINE);
  return s;
}

void buschain_reverb_destroy(BuschainReverbState *s) {
  if (!s) return;
  dl_free(&s->predelay_l);
  dl_free(&s->predelay_r);
  for (int i = 0; i < ER_TAPS; i++) dl_free(&s->er[i]);
  for (int i = 0; i < FDN_N; i++) dl_free(&s->fdn[i]);
  free(s);
}

/* Distance from origin to shell boundary along unit direction (room meters). */
static float shell_hit_dist(int room, float w, float h, float d, float dx, float dy, float dz) {
  float len = sqrtf(dx * dx + dy * dy + dz * dz);
  if (len < 1e-6f) return w;
  dx /= len;
  dy /= len;
  dz /= len;
  float t = 1.0e6f;
  switch (room) {
  case BUSCHAIN_ROOM_CYLINDER: {
    float r = fminf(w, d);
    float a = dx * dx + dz * dz;
    if (a > 1e-8f) {
      float disc = a * r * r;
      float th = sqrtf(disc) / a;
      if (th > 0.0f) t = fminf(t, th);
    }
    if (fabsf(dy) > 1e-6f) {
      float ty = (dy > 0.0f ? h : 0.0f) / dy;
      if (ty > 0.0f) t = fminf(t, ty);
    }
    break;
  }
  case BUSCHAIN_ROOM_TUNNEL: {
    /* Horizontal cylinder along Z */
    float r = fminf(w, h * 0.5f);
    float a = dx * dx + dy * dy;
    if (a > 1e-8f) {
      float th = r / sqrtf(a);
      if (th > 0.0f) t = fminf(t, th);
    }
    if (fabsf(dz) > 1e-6f) {
      float tz = (dz > 0.0f ? d : -d) / dz;
      if (tz > 0.0f) t = fminf(t, tz);
    }
    break;
  }
  case BUSCHAIN_ROOM_DOME:
  case BUSCHAIN_ROOM_BARREL: {
    /* Box walls + curved roof approx via taller hit */
    float tw = fabsf(dx) > 1e-6f ? w / fabsf(dx) : 1e6f;
    float td = fabsf(dz) > 1e-6f ? d / fabsf(dz) : 1e6f;
    float th = fabsf(dy) > 1e-6f ? ((dy > 0.0f ? h * 1.25f : 0.0f) / fabsf(dy)) : 1e6f;
    t = fminf(tw, fminf(td, th));
    break;
  }
  case BUSCHAIN_ROOM_CONE:
  case BUSCHAIN_ROOM_PYRAMID: {
    /* Tapered: radius shrinks with height */
    float tw = fabsf(dx) > 1e-6f ? w / fabsf(dx) : 1e6f;
    float td = fabsf(dz) > 1e-6f ? d / fabsf(dz) : 1e6f;
    float th = fabsf(dy) > 1e-6f ? h / fabsf(dy) : 1e6f;
    t = fminf(tw, fminf(td, th)) * 0.85f;
    break;
  }
  default: { /* shoebox */
    float tw = fabsf(dx) > 1e-6f ? w / fabsf(dx) : 1e6f;
    float th = fabsf(dy) > 1e-6f ? ((dy > 0.0f ? h : 0.01f) / fabsf(dy)) : 1e6f;
    float td = fabsf(dz) > 1e-6f ? d / fabsf(dz) : 1e6f;
    t = fminf(tw, fminf(th, td));
    break;
  }
  }
  return clampf(t, 0.2f, 40.0f);
}

static void redesign(BuschainReverbState *s) {
  float sr = (float)s->sr;
  float size = clampf(s->p.size, 0.1f, 4.0f);
  float shape = clampf(s->p.shape, 0.5f, 2.0f);
  float scale = size * (sr / 48000.0f);
  int room = (int)(s->p.room_type + 0.5f);
  if (room < 0) room = 0;
  if (room > 6) room = 6;

  float room_w = 4.0f * size * sqrtf(shape);
  float room_d = 5.0f * size / sqrtf(shape);
  float room_h = 2.8f * size;
  if (room == BUSCHAIN_ROOM_TUNNEL) {
    room_d *= 1.45f;
    room_w *= 0.75f;
  } else if (room == BUSCHAIN_ROOM_CYLINDER || room == BUSCHAIN_ROOM_DOME) {
    float r = fminf(room_w, room_d);
    room_w = room_d = r;
  }

  float cx = (clampf(s->p.source_x, 0.0f, 1.0f) * 2.0f - 1.0f) * room_w * 0.9f;
  float cy = clampf(s->p.source_y, 0.05f, 0.95f) * room_h;
  float cz = (clampf(s->p.source_z, 0.0f, 1.0f) * 2.0f - 1.0f) * room_d * 0.9f;
  float hx = (clampf(s->p.listener_x, 0.0f, 1.0f) * 2.0f - 1.0f) * room_w * 0.9f;
  float hy = clampf(s->p.listener_y, 0.05f, 0.95f) * room_h;
  float hz = (clampf(s->p.listener_z, 0.0f, 1.0f) * 2.0f - 1.0f) * room_d * 0.9f;

  float spacing = clampf(s->p.source_spacing, 0.0f, 1.0f);
  s->stereo_mix = spacing > 0.02f ? clampf((spacing - 0.02f) / 0.15f, 0.0f, 1.0f) : 0.0f;
  int face_lock = s->p.face_lock >= 0.5f;
  float yaw_rad = clampf(s->p.source_yaw, -180.0f, 180.0f) * ((float)M_PI / 180.0f);

  /* Speakers: forward toward head when face-locked. */
  float fwd_x, fwd_z;
  if (face_lock) {
    fwd_x = hx - cx;
    fwd_z = hz - cz;
    float fl = sqrtf(fwd_x * fwd_x + fwd_z * fwd_z);
    if (fl < 1e-4f) {
      fwd_x = 0.0f;
      fwd_z = 1.0f;
    } else {
      fwd_x /= fl;
      fwd_z /= fl;
    }
  } else {
    fwd_x = sinf(yaw_rad);
    fwd_z = cosf(yaw_rad);
  }
  float base_x = -fwd_z;
  float base_z = fwd_x;
  float half = spacing * fminf(room_w, room_d) * 0.45f;
  float spk[2][3] = {
      {cx - base_x * half, cy, cz - base_z * half},
      {cx + base_x * half, cy, cz + base_z * half},
  };

  /* Head faces speaker center; ears along ±interaural with Ear Angle flare. */
  float head_fx = cx - hx;
  float head_fz = cz - hz;
  float hfl = sqrtf(head_fx * head_fx + head_fz * head_fz);
  if (hfl < 1e-4f) {
    head_fx = -fwd_x;
    head_fz = -fwd_z;
    hfl = 1.0f;
  }
  head_fx /= hfl;
  head_fz /= hfl;
  float iax = -head_fz; /* left ear +interaural */
  float iaz = head_fx;
  float ear_sp = clampf(s->p.listener_spacing, 0.0f, 1.0f);
  float ear_half = (0.04f + ear_sp * 0.14f) * fminf(room_w, room_d); /* ~human→wide */
  float ear_ang = clampf(s->p.ear_angle, 90.0f, 180.0f) * ((float)M_PI / 180.0f);
  /* 180° → pure ±IA; smaller → each ear faces forward of lateral by (180-ang)/2. */
  float flare = (float)M_PI * 0.5f - ear_ang * 0.5f;
  float ear[2][3] = {
      {hx + iax * ear_half, hy, hz + iaz * ear_half},
      {hx - iax * ear_half, hy, hz - iaz * ear_half},
  };
  /* Facing normals (for ILD shadow): rotate IA toward head forward by flare. */
  float ear_nx[2], ear_nz[2];
  ear_nx[0] = iax * cosf(flare) + head_fx * sinf(flare);
  ear_nz[0] = iaz * cosf(flare) + head_fz * sinf(flare);
  ear_nx[1] = -iax * cosf(flare) + head_fx * sinf(flare);
  ear_nz[1] = -iaz * cosf(flare) + head_fz * sinf(flare);

  float c_sound = 343.0f;
  float sl_dist = 0.0f;
  for (int e = 0; e < 2; e++)
    for (int p = 0; p < 2; p++) {
      float dx = ear[e][0] - spk[p][0];
      float dy = ear[e][1] - spk[p][1];
      float dz = ear[e][2] - spk[p][2];
      sl_dist += sqrtf(dx * dx + dy * dy + dz * dz);
    }
  sl_dist *= 0.25f;

  for (int i = 0; i < ER_TAPS; i++) {
    int spk_i = (i >> 1) & 1; /* L L R R L L … */
    int ear_i = i & 1;        /* route to alternating ears → clear ITD */
    s->er_in_ch[i] = spk_i;
    s->er_out_ch[i] = ear_i;
    float sx = spk[spk_i][0];
    float sy = spk[spk_i][1];
    float sz = spk[spk_i][2];
    float ex = ear[ear_i][0];
    float ey = ear[ear_i][1];
    float ez = ear[ear_i][2];

    float dist;
    if (i < 4) {
      /* Direct path (no bounce) — dominant listener-position cue */
      float ddx = ex - sx, ddy = ey - sy, ddz = ez - sz;
      dist = sqrtf(ddx * ddx + ddy * ddy + ddz * ddz);
    } else {
      float ang = (float)i * (2.0f * (float)M_PI / (float)ER_TAPS) + s->p.er_spread * 0.7f;
      float elev = -0.35f + 0.7f * ((float)(i % 5) / 4.0f);
      float dx = cosf(ang) * cosf(elev);
      float dy = sinf(elev);
      float dz = sinf(ang) * cosf(elev);
      float t_hit = shell_hit_dist(room, room_w, room_h, room_d, dx, dy, dz);
      float bx = sx + dx * t_hit;
      float by = clampf(sy + dy * t_hit, 0.0f, room_h);
      float bz = sz + dz * t_hit;
      float d1 = sqrtf((bx - sx) * (bx - sx) + (by - sy) * (by - sy) + (bz - sz) * (bz - sz));
      float d2 = sqrtf((ex - bx) * (ex - bx) + (ey - by) * (ey - by) + (ez - bz) * (ez - bz));
      dist = d1 + d2;
      if (room == BUSCHAIN_ROOM_SHOEBOX) {
        int ix = (i % 3) - 1;
        int iy = ((i / 3) % 3) - 1;
        if (ix == 0 && iy == 0) ix = 1;
        float idx =
            room_w * (float)(ix == 0 ? 1 : (ix > 0 ? ix : -ix)) - sx * (float)(ix != 0 ? 1 : 0);
        float idy = room_h * (float)(iy == 0 ? 1 : (iy > 0 ? iy : -iy));
        float idz = room_d * (1.0f + 0.25f * (float)(i % 4)) - sz;
        float idist = sqrtf(idx * idx + idy * idy + idz * idz);
        dist = 0.55f * dist + 0.45f * idist;
      }
    }

    /* ILD: attenuate when arrival is behind the ear facing */
    float to_ex = ex - sx, to_ez = ez - sz;
    float tl = sqrtf(to_ex * to_ex + to_ez * to_ez);
    float shadow = 1.0f;
    if (tl > 1e-4f) {
      float cos_a = (to_ex * ear_nx[ear_i] + to_ez * ear_nz[ear_i]) / tl;
      shadow = 0.35f + 0.65f * clampf(0.5f + 0.5f * cos_a, 0.0f, 1.0f);
      if (spk_i != ear_i) shadow *= 0.72f; /* contralateral */
    }

    float ms = (dist / c_sound) * 1000.0f * (0.65f + s->p.er_spread * 0.7f);
    s->er_delays[i] = clampf(ms * 0.001f * sr, 4.0f, (float)(MAX_PREDELAY + 4000));
    float g = (0.62f / (0.25f + dist * 0.28f)) * (1.0f - 0.03f * (float)i) * shadow;
    if (i < 4) g *= 1.35f; /* emphasize direct ITD/ILD */
    s->er_gains[i] = g;
  }

  float dens = 0.65f + s->p.density * 0.55f;
  float path_bias = 1.0f + clampf(sl_dist / (room_w + room_d + 0.1f), 0.0f, 1.0f) * 0.18f;
  float path_ear_l =
      0.5f * (sqrtf((ear[0][0] - spk[0][0]) * (ear[0][0] - spk[0][0]) +
                    (ear[0][2] - spk[0][2]) * (ear[0][2] - spk[0][2])) +
              sqrtf((ear[0][0] - spk[1][0]) * (ear[0][0] - spk[1][0]) +
                    (ear[0][2] - spk[1][2]) * (ear[0][2] - spk[1][2])));
  float path_ear_r =
      0.5f * (sqrtf((ear[1][0] - spk[0][0]) * (ear[1][0] - spk[0][0]) +
                    (ear[1][2] - spk[0][2]) * (ear[1][2] - spk[0][2])) +
              sqrtf((ear[1][0] - spk[1][0]) * (ear[1][0] - spk[1][0]) +
                    (ear[1][2] - spk[1][2]) * (ear[1][2] - spk[1][2])));
  for (int i = 0; i < FDN_N; i++) {
    float pth = (i < FDN_N / 2) ? path_ear_l : path_ear_r;
    float ch_bias = 1.0f + clampf(pth / (room_w + room_d + 0.1f), 0.0f, 1.0f) * 0.22f;
    float bias = (i < 3) ? path_bias : 1.0f;
    s->fdn_len[i] =
        clampf((float)BASE_DELAYS[i] * scale * dens * bias * ch_bias, 64.0f, (float)(MAX_LINE - 8));
    float cut = 2000.0f + (1.0f - clampf(s->p.decay_hi, 0.25f, 2.0f) * 0.5f) * 10000.0f;
    cut *= 0.6f + s->p.character * 0.7f;
    f1_lp(&s->damp[i], cut, sr);
  }

  f1_hp(&s->wet_hp_l, s->p.wet_hp_hz, sr);
  f1_hp(&s->wet_hp_r, s->p.wet_hp_hz, sr);
  f1_lp(&s->wet_lp_l, s->p.wet_lp_hz, sr);
  f1_lp(&s->wet_lp_r, s->p.wet_lp_hz, sr);
}

void buschain_reverb_reset(BuschainReverbState *s, double sample_rate) {
  if (!s) return;
  s->sr = sample_rate < 8000.0 ? 48000.0 : sample_rate;
  dl_clear(&s->predelay_l);
  dl_clear(&s->predelay_r);
  for (int i = 0; i < ER_TAPS; i++) dl_clear(&s->er[i]);
  for (int i = 0; i < FDN_N; i++) {
    dl_clear(&s->fdn[i]);
    s->lfo_phase[i] = (float)i * 0.7f;
    s->damp[i].xz = s->damp[i].yz = 0.0f;
  }
  s->wet_hp_l = s->wet_hp_r = (Filter1){0};
  s->wet_lp_l = s->wet_lp_r = (Filter1){0};
  s->duck_env = 0.0f;
  s->gate_env = 1.0f;
  s->sm_mix = s->p.mix;
  s->sm_size = s->p.size;
  s->sm_rt60 = s->p.rt60;
  s->energy_env = 0.0f;
  s->er_energy = 0.0f;
  s->tail_energy = 0.0f;
  s->sample_count = 0;
  memset(&s->meters, 0, sizeof(s->meters));
  redesign(s);
}

void buschain_reverb_set_params(BuschainReverbState *s, const BuschainReverbParams *p) {
  if (!s || !p) return;
  int redesign_needed =
      fabsf(p->size - s->p.size) > 1e-4f || fabsf(p->shape - s->p.shape) > 1e-4f ||
      fabsf(p->density - s->p.density) > 1e-4f || fabsf(p->er_spread - s->p.er_spread) > 1e-4f ||
      fabsf(p->decay_hi - s->p.decay_hi) > 1e-4f || fabsf(p->character - s->p.character) > 1e-4f ||
      fabsf(p->wet_hp_hz - s->p.wet_hp_hz) > 0.5f || fabsf(p->wet_lp_hz - s->p.wet_lp_hz) > 0.5f ||
      fabsf(p->room_type - s->p.room_type) > 0.1f ||
      fabsf(p->source_x - s->p.source_x) > 1e-4f || fabsf(p->source_y - s->p.source_y) > 1e-4f ||
      fabsf(p->source_z - s->p.source_z) > 1e-4f || fabsf(p->listener_x - s->p.listener_x) > 1e-4f ||
      fabsf(p->listener_y - s->p.listener_y) > 1e-4f || fabsf(p->listener_z - s->p.listener_z) > 1e-4f ||
      fabsf(p->source_spacing - s->p.source_spacing) > 1e-4f ||
      fabsf(p->source_yaw - s->p.source_yaw) > 0.05f ||
      fabsf(p->face_lock - s->p.face_lock) > 0.1f ||
      fabsf(p->listener_spacing - s->p.listener_spacing) > 1e-4f ||
      fabsf(p->ear_angle - s->p.ear_angle) > 0.1f;
  s->p = *p;
  if (redesign_needed) redesign(s);
}

/* Householder-ish feedback: y = x - (2/N) * sum(x)  (orthogonal-ish, cheap) */
static void fdn_mix(const float in[FDN_N], float out[FDN_N], float diffusion) {
  float sum = 0.0f;
  for (int i = 0; i < FDN_N; i++) sum += in[i];
  float k = (2.0f / (float)FDN_N) * (0.55f + diffusion * 0.45f);
  for (int i = 0; i < FDN_N; i++) out[i] = in[i] - k * sum;
}

void buschain_reverb_process(BuschainReverbState *s,
                             const float *in_l, const float *in_r,
                             float *out_l, float *out_r,
                             uint32_t n) {
  if (!s || !in_l || !out_l) return;
  if (!in_r) in_r = in_l;
  if (!out_r) out_r = out_l;

  if (s->p.bypass) {
    if (out_l != in_l) memcpy(out_l, in_l, n * sizeof(float));
    if (out_r != in_r) memcpy(out_r, in_r, n * sizeof(float));
    return;
  }

  float sr = (float)s->sr;
  float sc = ms_to_coeff(20.0f, sr);
  float peak = 0.0f;
  float er_acc = 0.0f, tail_acc = 0.0f;

  float g_target = 0.0f;
  {
    /* Feedback gain for RT60: g = 10^(-3*delay/rt60) */
    float rt = clampf(s->p.rt60, 0.1f, 12.0f) * clampf(s->p.decay_lo, 0.25f, 2.0f);
    float avg_len = 0.0f;
    for (int i = 0; i < FDN_N; i++) avg_len += s->fdn_len[i];
    avg_len /= (float)FDN_N;
    float t = avg_len / sr;
    g_target = powf(10.0f, -3.0f * t / rt);
    g_target = clampf(g_target, 0.0f, 0.9995f);
  }
  float freeze = s->p.freeze >= 0.5f ? 1.0f : 0.0f;
  if (freeze > 0.5f) g_target = 0.9998f;

  float mod_amt = clampf(s->p.modulation, 0.0f, 1.0f) * 0.35f;
  float width = clampf(s->p.width, 0.0f, 1.0f);
  float duck_amt = clampf(s->p.duck_amount, 0.0f, 1.0f);
  float duck_rel = ms_to_coeff(clampf(s->p.duck_release_ms, 10.0f, 1000.0f), sr);
  float duck_atk = ms_to_coeff(10.0f, sr);

  for (uint32_t n_i = 0; n_i < n; n_i++) {
    float dry_l = in_l[n_i];
    float dry_r = in_r[n_i];
    float mono = 0.5f * (dry_l + dry_r);
    float st = s->stereo_mix;
    float feed_l = mono * (1.0f - st) + dry_l * st;
    float feed_r = mono * (1.0f - st) + dry_r * st;

    smooth_toward(&s->sm_mix, s->p.mix, sc);
    smooth_toward(&s->sm_rt60, s->p.rt60, sc);

    /* Duck envelope from dry */
    float dry_abs = fabsf(mono);
    if (dry_abs > s->duck_env)
      smooth_toward(&s->duck_env, dry_abs, duck_atk);
    else
      smooth_toward(&s->duck_env, dry_abs, duck_rel);
    float duck_gr = 1.0f - duck_amt * clampf(s->duck_env * 4.0f, 0.0f, 1.0f);

    /* Gate: after silence, fade wet */
    if (s->p.gate_time_ms > 1.0f && freeze < 0.5f) {
      if (dry_abs > 0.002f)
        s->gate_env = 1.0f;
      else {
        float gcoeff = ms_to_coeff(s->p.gate_time_ms, sr);
        smooth_toward(&s->gate_env, 0.0f, gcoeff);
      }
    } else {
      s->gate_env = 1.0f;
    }

    float pd = clampf(s->p.predelay_ms, 0.0f, 200.0f) * 0.001f * sr;
    if (pd < 1.0f) pd = 1.0f;
    dl_write(&s->predelay_l, feed_l);
    dl_write(&s->predelay_r, feed_r);
    float pred_l = dl_read(&s->predelay_l, pd);
    float pred_r = dl_read(&s->predelay_r, pd);

    /* Early reflections — speaker feed × ear receive (ITD/ILD) */
    float er = 0.0f;
    float er_l = 0.0f, er_r = 0.0f;
    for (int i = 0; i < ER_TAPS; i++) {
      float pred = s->er_in_ch[i] ? pred_r : pred_l;
      dl_write(&s->er[i], pred);
      float t = dl_read(&s->er[i], s->er_delays[i]);
      float g = s->er_gains[i] * s->p.er_level;
      er += t * g;
      if (s->er_out_ch[i])
        er_r += t * g;
      else
        er_l += t * g;
    }
    er_acc += fabsf(er);

    /* FDN — L lines biased by left inject, R by right */
    float u[FDN_N], v[FDN_N];
    float inj_scale = (0.15f + s->p.diffusion * 0.1f) * (1.0f - freeze * 0.85f);
    float inj_l = pred_l * inj_scale;
    float inj_r = pred_r * inj_scale;
    for (int i = 0; i < FDN_N; i++) {
      s->lfo_phase[i] += (0.11f + 0.03f * (float)i) / sr;
      if (s->lfo_phase[i] > 1.0f) s->lfo_phase[i] -= 1.0f;
      float lfo = sinf(s->lfo_phase[i] * (float)(2.0 * M_PI));
      float dly = s->fdn_len[i] * (1.0f + mod_amt * 0.012f * lfo);
      float x = dl_read(&s->fdn[i], dly);
      x = f1_tick(&s->damp[i], x);
      float lo_scale = 0.85f + 0.15f * clampf(s->p.decay_lo, 0.25f, 2.0f);
      u[i] = x * g_target * lo_scale;
    }
    fdn_mix(u, v, s->p.diffusion);
    float late = 0.0f;
    float late_l = 0.0f, late_r = 0.0f;
    for (int i = 0; i < FDN_N; i++) {
      float inj = (i < FDN_N / 2) ? inj_l : inj_r;
      float in_i = v[i] + inj * ((i & 1) ? 1.0f : 0.85f);
      if (freeze > 0.5f) in_i = u[i] * 0.9998f + inj * 0.02f;
      dl_write(&s->fdn[i], in_i);
      late += in_i;
      if (i < FDN_N / 2) late_l += in_i;
      else late_r += in_i;
    }
    late *= 0.35f;
    late_l *= 0.45f;
    late_r *= 0.45f;
    tail_acc += fabsf(late);

    float wet_l = er_l + late_l * (1.0f - s->p.er_level * 0.25f);
    float wet_r = er_r + late_r * (1.0f - s->p.er_level * 0.25f);
    /* Width */
    float mid = 0.5f * (wet_l + wet_r);
    float side = 0.5f * (wet_l - wet_r) * (0.3f + width * 1.4f);
    wet_l = mid + side;
    wet_r = mid - side;

    wet_l = f1_tick(&s->wet_lp_l, f1_tick(&s->wet_hp_l, wet_l));
    wet_r = f1_tick(&s->wet_lp_r, f1_tick(&s->wet_hp_r, wet_r));

    wet_l *= duck_gr * s->gate_env;
    wet_r *= duck_gr * s->gate_env;

    float m = clampf(s->sm_mix, 0.0f, 1.0f);
    out_l[n_i] = dry_l * (1.0f - m) + wet_l * m;
    out_r[n_i] = dry_r * (1.0f - m) + wet_r * m;

    float wp = fabsf(wet_l);
    if (fabsf(wet_r) > wp) wp = fabsf(wet_r);
    if (wp > peak) peak = wp;

    s->sample_count++;
  }

  /* Meters */
  float inv = 1.0f / (float)(n > 0 ? n : 1);
  s->er_energy = 0.9f * s->er_energy + 0.1f * er_acc * inv;
  s->tail_energy = 0.9f * s->tail_energy + 0.1f * tail_acc * inv;
  float tot = s->er_energy + s->tail_energy + 1e-8f;
  s->meters.er_tail = s->er_energy / tot;
  s->meters.echo_density = clampf(s->p.density * 0.5f + s->p.diffusion * 0.5f + s->tail_energy * 2.0f, 0.0f, 1.0f);
  s->meters.rt60_est = s->sm_rt60;
  s->meters.band_t60_lo = s->sm_rt60 * clampf(s->p.decay_lo, 0.25f, 2.0f);
  s->meters.band_t60_mid = s->sm_rt60;
  s->meters.band_t60_hi = s->sm_rt60 * clampf(s->p.decay_hi, 0.25f, 2.0f);
  s->meters.wet_peak = peak;
  s->meters.duck_gr =
      clampf(s->p.duck_amount * clampf(s->duck_env * 4.0f, 0.0f, 1.0f), 0.0f, 1.0f);
}

void buschain_reverb_get_meters(const BuschainReverbState *s, BuschainReverbMeters *m) {
  if (!s || !m) return;
  *m = s->meters;
}
