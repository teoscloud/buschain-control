/* BusChain Gate — LV2 wrapper */
#include "dsp.h"

#include <lv2/core/lv2.h>

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define BUSCHAIN_GATE_URI "https://github.com/local/buschain_gate#gate"

typedef enum {
  P_INPUT_L = 0,
  P_INPUT_R,
  P_OUTPUT_L,
  P_OUTPUT_R,
  P_ENABLE,
  P_THRESHOLD,
  P_HYST,
  P_ATTACK,
  P_HOLD,
  P_RELEASE,
  P_RANGE,
  P_MIX,
  P_BYPASS,
  P_METER_GAIN,
  P_N_PORTS
} PortIndex;

typedef struct {
  const float *ports[P_N_PORTS];
  float *out_ports[P_N_PORTS];
  BuschainGateState *dsp;
  double sr;
} BuschainGateLV2;

static LV2_Handle instantiate(const LV2_Descriptor *descriptor,
                              double rate,
                              const char *bundle_path,
                              const LV2_Feature *const *features) {
  (void)descriptor;
  (void)bundle_path;
  (void)features;
  BuschainGateLV2 *self = (BuschainGateLV2 *)calloc(1, sizeof(BuschainGateLV2));
  if (!self) return NULL;
  self->dsp = buschain_gate_create();
  if (!self->dsp) {
    free(self);
    return NULL;
  }
  self->sr = rate;
  buschain_gate_reset(self->dsp, rate);
  return (LV2_Handle)self;
}

static void connect_port(LV2_Handle instance, uint32_t port, void *data) {
  BuschainGateLV2 *self = (BuschainGateLV2 *)instance;
  if (port >= P_N_PORTS) return;
  self->ports[port] = (const float *)data;
  self->out_ports[port] = (float *)data;
}

static float pget(BuschainGateLV2 *self, PortIndex p, float def) {
  return self->ports[p] ? *self->ports[p] : def;
}

static void activate(LV2_Handle instance) {
  BuschainGateLV2 *self = (BuschainGateLV2 *)instance;
  buschain_gate_reset(self->dsp, self->sr);
}

static void run(LV2_Handle instance, uint32_t n_samples) {
  BuschainGateLV2 *self = (BuschainGateLV2 *)instance;
  BuschainGateParams params;
  buschain_gate_default_params(&params);

  params.enable = pget(self, P_ENABLE, (float)params.enable) >= 0.5f;
  params.threshold_db = pget(self, P_THRESHOLD, params.threshold_db);
  params.hysteresis_db = pget(self, P_HYST, params.hysteresis_db);
  params.attack_ms = pget(self, P_ATTACK, params.attack_ms);
  params.hold_ms = pget(self, P_HOLD, params.hold_ms);
  params.release_ms = pget(self, P_RELEASE, params.release_ms);
  params.range_db = pget(self, P_RANGE, params.range_db);
  params.mix = pget(self, P_MIX, params.mix);
  params.bypass = pget(self, P_BYPASS, 0.0f) >= 0.5f;

  buschain_gate_set_params(self->dsp, &params);

  const float *in_l = self->ports[P_INPUT_L];
  const float *in_r = self->ports[P_INPUT_R];
  float *out_l = self->out_ports[P_OUTPUT_L];
  float *out_r = self->out_ports[P_OUTPUT_R];
  if (!in_l || !out_l) return;
  if (!in_r) in_r = in_l;
  if (!out_r) out_r = out_l;

  buschain_gate_process(self->dsp, in_l, in_r, out_l, out_r, n_samples);

  if (self->out_ports[P_METER_GAIN])
    *self->out_ports[P_METER_GAIN] = buschain_gate_meter_gain(self->dsp);
}

static void cleanup(LV2_Handle instance) {
  BuschainGateLV2 *self = (BuschainGateLV2 *)instance;
  if (self) {
    buschain_gate_destroy(self->dsp);
    free(self);
  }
}

static const LV2_Descriptor descriptor = {
  BUSCHAIN_GATE_URI,
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
