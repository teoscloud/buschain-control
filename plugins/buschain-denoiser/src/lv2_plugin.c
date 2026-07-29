/* BUSCHAIN_DN Multiband Denoiser — LV2 wrapper */
#include "dsp.h"

#include <lv2/core/lv2.h>

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define BUSCHAIN_DN_URI "https://github.com/local/buschain_dn#denoiser"

typedef enum {
  P_INPUT_L = 0,
  P_INPUT_R,
  P_OUTPUT_L,
  P_OUTPUT_R,
  P_THRESHOLD,
  P_RANGE0,
  P_RANGE1,
  P_RANGE2,
  P_RANGE3,
  P_RANGE4,
  P_RANGE5,
  P_FREQ_LOW,
  P_FREQ_HIGH,
  P_HF_BIAS,
  P_STEREO_LINK,
  P_ATTACK,
  P_RELEASE,
  P_KNEE,
  P_RATIO,
  P_HPF,
  P_LPF,
  P_BYPASS,
  P_METER_IN_PEAK,
  P_METER_IN_MIN,
  P_METER_GR0,
  P_METER_GR1,
  P_METER_GR2,
  P_METER_GR3,
  P_METER_GR4,
  P_METER_GR5,
  P_N_PORTS
} PortIndex;

typedef struct {
  const float *ports[P_N_PORTS];
  float *out_ports[P_N_PORTS];
  BuschainDnState *dsp;
  double sr;
} BuschainDnLV2;

static LV2_Handle instantiate(const LV2_Descriptor *descriptor,
                              double rate,
                              const char *bundle_path,
                              const LV2_Feature *const *features) {
  (void)descriptor;
  (void)bundle_path;
  (void)features;
  BuschainDnLV2 *self = (BuschainDnLV2 *)calloc(1, sizeof(BuschainDnLV2));
  if (!self) return NULL;
  self->dsp = buschain_dn_create();
  if (!self->dsp) {
    free(self);
    return NULL;
  }
  self->sr = rate;
  buschain_dn_reset(self->dsp, rate);
  return (LV2_Handle)self;
}

static void connect_port(LV2_Handle instance, uint32_t port, void *data) {
  BuschainDnLV2 *self = (BuschainDnLV2 *)instance;
  if (port >= P_N_PORTS) return;
  self->ports[port] = (const float *)data;
  self->out_ports[port] = (float *)data;
}

static float pget(BuschainDnLV2 *self, PortIndex p, float def) {
  return self->ports[p] ? *self->ports[p] : def;
}

static void activate(LV2_Handle instance) {
  BuschainDnLV2 *self = (BuschainDnLV2 *)instance;
  buschain_dn_reset(self->dsp, self->sr);
}

static void run(LV2_Handle instance, uint32_t n_samples) {
  BuschainDnLV2 *self = (BuschainDnLV2 *)instance;
  BuschainDnParams params;
  buschain_dn_default_params(&params);

  params.threshold_db = pget(self, P_THRESHOLD, params.threshold_db);
  params.range_db[0] = pget(self, P_RANGE0, params.range_db[0]);
  params.range_db[1] = pget(self, P_RANGE1, params.range_db[1]);
  params.range_db[2] = pget(self, P_RANGE2, params.range_db[2]);
  params.range_db[3] = pget(self, P_RANGE3, params.range_db[3]);
  params.range_db[4] = pget(self, P_RANGE4, params.range_db[4]);
  params.range_db[5] = pget(self, P_RANGE5, params.range_db[5]);
  params.freq_low_hz = pget(self, P_FREQ_LOW, params.freq_low_hz);
  params.freq_high_hz = pget(self, P_FREQ_HIGH, params.freq_high_hz);
  params.hf_bias = pget(self, P_HF_BIAS, params.hf_bias);
  params.stereo_link = pget(self, P_STEREO_LINK, params.stereo_link);
  params.attack_ms = pget(self, P_ATTACK, params.attack_ms);
  params.release_ms = pget(self, P_RELEASE, params.release_ms);
  params.knee_db = pget(self, P_KNEE, params.knee_db);
  params.ratio = pget(self, P_RATIO, params.ratio);
  params.hpf_enable = pget(self, P_HPF, 0.0f) >= 0.5f;
  params.lpf_enable = pget(self, P_LPF, 0.0f) >= 0.5f;
  params.bypass = pget(self, P_BYPASS, 0.0f) >= 0.5f;

  buschain_dn_set_params(self->dsp, &params);

  const float *in_l = self->ports[P_INPUT_L];
  const float *in_r = self->ports[P_INPUT_R];
  float *out_l = self->out_ports[P_OUTPUT_L];
  float *out_r = self->out_ports[P_OUTPUT_R];
  if (!in_l || !out_l) return;
  if (!in_r) in_r = in_l;
  if (!out_r) out_r = out_l;

  buschain_dn_process(self->dsp, in_l, in_r, out_l, out_r, n_samples);

  if (self->out_ports[P_METER_IN_PEAK])
    *self->out_ports[P_METER_IN_PEAK] = buschain_dn_meter_input_peak_db(self->dsp);
  if (self->out_ports[P_METER_IN_MIN])
    *self->out_ports[P_METER_IN_MIN] = buschain_dn_meter_input_min_db(self->dsp);
  for (int b = 0; b < BUSCHAIN_DN_BANDS; b++) {
    if (self->out_ports[P_METER_GR0 + b])
      *self->out_ports[P_METER_GR0 + b] = buschain_dn_meter_gr_min_db(self->dsp, b);
  }
}

static void cleanup(LV2_Handle instance) {
  BuschainDnLV2 *self = (BuschainDnLV2 *)instance;
  if (self) {
    buschain_dn_destroy(self->dsp);
    free(self);
  }
}

static const LV2_Descriptor descriptor = {
  BUSCHAIN_DN_URI,
  instantiate,
  connect_port,
  activate,
  run,
  NULL,
  cleanup,
  NULL
};

LV2_SYMBOL_EXPORT
const LV2_Descriptor *lv2_descriptor(uint32_t index) {
  return index == 0 ? &descriptor : NULL;
}
