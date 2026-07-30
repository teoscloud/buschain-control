/* Offline DSP smoke test — generate noise + tone, process, print meters */
#include "dsp.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

int main(void) {
  const double sr = 48000.0;
  const uint32_t n = 48000;
  float *in_l = calloc(n, sizeof(float));
  float *in_r = calloc(n, sizeof(float));
  float *out_l = calloc(n, sizeof(float));
  float *out_r = calloc(n, sizeof(float));
  if (!in_l || !in_r || !out_l || !out_r) return 1;

  /* Quiet hiss + tone bursts with long noise gaps (lets DD hangover settle). */
  unsigned seed = 1;
  const uint32_t burst = 8000;   /* ~167 ms tone */
  const uint32_t gap = 24000;    /* ~500 ms noise-only */
  const uint32_t period = burst + gap;
  for (uint32_t i = 0; i < n; i++) {
    seed = seed * 1664525u + 1013904223u;
    float noise = ((seed >> 8) / 16777215.0f) * 2.0f - 1.0f;
    noise *= 0.02f; /* ~-34 dBFS noise floor */
    float tone = 0.0f;
    if ((i % period) < burst) {
      tone = 0.25f * sinf(2.0f * (float)M_PI * 1000.0f * (float)i / (float)sr);
    }
    in_l[i] = noise + tone;
    in_r[i] = noise * 0.9f + tone * 0.8f;
  }

  BuschainDnState *s = buschain_dn_create();
  buschain_dn_reset(s, sr);
  BuschainDnParams p;
  buschain_dn_default_params(&p);
  p.threshold_db = -40.0f;
  for (int b = 0; b < BUSCHAIN_DN_BANDS; b++)
    p.range_db[b] = 24.0f;
  buschain_dn_set_params(s, &p);
  buschain_dn_process(s, in_l, in_r, out_l, out_r, n);

  double in_e = 0, out_e = 0;
  double in_noise = 0, out_noise = 0;
  uint32_t n_noise = 0;
  float peak_tone_in = 0.0f, peak_tone_out = 0.0f;
  /* Skip first period (cold start / learning), then score. */
  for (uint32_t i = period; i < n; i++) {
    in_e += (double)in_l[i] * in_l[i];
    out_e += (double)out_l[i] * out_l[i];
    int tone_on = ((i % period) < burst);
    if (tone_on && (i % period) > 2000) {
      /* Skip burst attack — score steady-state tone peak. */
      float ai = fabsf(in_l[i]);
      float ao = fabsf(out_l[i]);
      if (ai > peak_tone_in) peak_tone_in = ai;
      if (ao > peak_tone_out) peak_tone_out = ao;
    } else if ((i % period) > burst + 4000) {
      /* Late in gap — after DD hangover. */
      in_noise += (double)in_l[i] * in_l[i];
      out_noise += (double)out_l[i] * out_l[i];
      n_noise++;
    }
  }
  uint32_t n_scored = n - period;
  double red = 20.0 * log10((sqrt(out_e / n_scored) + 1e-12) /
                            (sqrt(in_e / n_scored) + 1e-12));
  double red_n = 20.0 * log10((sqrt(out_noise / n_noise) + 1e-12) /
                              (sqrt(in_noise / n_noise) + 1e-12));
  double tone_peak_db = 20.0 * log10((peak_tone_out + 1e-12) / (peak_tone_in + 1e-12));
  printf("ok  in_rms=%.6f out_rms=%.6f reduction_db=%.2f\n",
         sqrt(in_e / n_scored), sqrt(out_e / n_scored), red);
  printf("    noise_gap=%.1f dB  tone_peak=%.1f dB  (want noise dug, tone kept)\n",
         red_n, tone_peak_db);
  printf("    peak_in=%.1f dB  gr_band5=%.1f dB\n",
         buschain_dn_meter_input_peak_db(s), buschain_dn_meter_gr_min_db(s, 5));

  buschain_dn_destroy(s);
  free(in_l); free(in_r); free(out_l); free(out_r);
  if (!(red_n < -5.0)) {
    fprintf(stderr, "fail: expected noise-gap reduction < -5 dB\n");
    return 2;
  }
  if (!(tone_peak_db > -5.0)) {
    fprintf(stderr, "fail: expected tone peak loss > -5 dB (less attenuation)\n");
    return 3;
  }
  return 0;
}
