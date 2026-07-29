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

  /* Quiet hiss + occasional tone bursts (game-like: sparse content in noise) */
  unsigned seed = 1;
  for (uint32_t i = 0; i < n; i++) {
    seed = seed * 1664525u + 1013904223u;
    float noise = ((seed >> 8) / 16777215.0f) * 2.0f - 1.0f;
    noise *= 0.02f; /* ~-34 dBFS noise floor */
    float tone = 0.0f;
    if ((i / 6000) % 2 == 0) {
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
  buschain_dn_set_params(s, &p);
  buschain_dn_process(s, in_l, in_r, out_l, out_r, n);

  double in_e = 0, out_e = 0;
  for (uint32_t i = 0; i < n; i++) {
    in_e += (double)in_l[i] * in_l[i];
    out_e += (double)out_l[i] * out_l[i];
  }
  printf("ok  in_rms=%.6f out_rms=%.6f reduction_db=%.2f\n",
         sqrt(in_e / n), sqrt(out_e / n),
         20.0 * log10((sqrt(out_e / n) + 1e-12) / (sqrt(in_e / n) + 1e-12)));
  printf("    peak_in=%.1f dB  gr_band5=%.1f dB\n",
         buschain_dn_meter_input_peak_db(s), buschain_dn_meter_gr_min_db(s, 5));

  buschain_dn_destroy(s);
  free(in_l); free(in_r); free(out_l); free(out_r);
  return 0;
}
