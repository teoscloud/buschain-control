/* BusChain Gate — LADSPA wrapper (PipeWire filter-chain friendly) */
#include "dsp.h"
#include "ladspa.h"

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define BUSCHAIN_GATE_LADSPA_UNIQUE_ID 392002

enum {
  L_INPUT_L = 0,
  L_INPUT_R,
  L_OUTPUT_L,
  L_OUTPUT_R,
  L_ENABLE,
  L_THRESHOLD,
  L_HYST,
  L_ATTACK,
  L_HOLD,
  L_RELEASE,
  L_RANGE,
  L_MIX,
  L_BYPASS,
  L_N_PORTS
};

typedef struct {
  const LADSPA_Data *ports[L_N_PORTS];
  LADSPA_Data *outs[L_N_PORTS];
  BuschainGateState *dsp;
  unsigned long sr;
} BuschainGateLADSPA;

static LADSPA_Handle instantiate(const LADSPA_Descriptor *desc, unsigned long sample_rate) {
  (void)desc;
  BuschainGateLADSPA *self = (BuschainGateLADSPA *)calloc(1, sizeof(BuschainGateLADSPA));
  if (!self) return NULL;
  self->dsp = buschain_gate_create();
  if (!self->dsp) {
    free(self);
    return NULL;
  }
  self->sr = sample_rate;
  buschain_gate_reset(self->dsp, (double)sample_rate);
  return self;
}

static void connect_port(LADSPA_Handle instance, unsigned long port, LADSPA_Data *data) {
  BuschainGateLADSPA *self = (BuschainGateLADSPA *)instance;
  if (port >= L_N_PORTS) return;
  self->ports[port] = data;
  self->outs[port] = data;
}

static void activate(LADSPA_Handle instance) {
  BuschainGateLADSPA *self = (BuschainGateLADSPA *)instance;
  buschain_gate_reset(self->dsp, (double)self->sr);
}

static float pget(BuschainGateLADSPA *self, int p, float def) {
  return self->ports[p] ? (float)(*self->ports[p]) : def;
}

static void run(LADSPA_Handle instance, unsigned long n_samples) {
  BuschainGateLADSPA *self = (BuschainGateLADSPA *)instance;
  BuschainGateParams params;
  buschain_gate_default_params(&params);

  params.enable = pget(self, L_ENABLE, (float)params.enable) >= 0.5f;
  params.threshold_db = pget(self, L_THRESHOLD, params.threshold_db);
  params.hysteresis_db = pget(self, L_HYST, params.hysteresis_db);
  params.attack_ms = pget(self, L_ATTACK, params.attack_ms);
  params.hold_ms = pget(self, L_HOLD, params.hold_ms);
  params.release_ms = pget(self, L_RELEASE, params.release_ms);
  params.range_db = pget(self, L_RANGE, params.range_db);
  params.mix = pget(self, L_MIX, params.mix);
  params.bypass = pget(self, L_BYPASS, 0.0f) >= 0.5f;

  buschain_gate_set_params(self->dsp, &params);

  const float *in_l = self->ports[L_INPUT_L];
  const float *in_r = self->ports[L_INPUT_R];
  float *out_l = self->outs[L_OUTPUT_L];
  float *out_r = self->outs[L_OUTPUT_R];
  if (!in_l || !out_l) return;
  if (!in_r) in_r = in_l;
  if (!out_r) out_r = out_l;

  buschain_gate_process(self->dsp, in_l, in_r, out_l, out_r, (uint32_t)n_samples);
}

static void cleanup(LADSPA_Handle instance) {
  BuschainGateLADSPA *self = (BuschainGateLADSPA *)instance;
  if (self) {
    buschain_gate_destroy(self->dsp);
    free(self);
  }
}

static LADSPA_PortDescriptor port_descriptors[L_N_PORTS];
static LADSPA_PortRangeHint port_hints[L_N_PORTS];
static const char *port_names[L_N_PORTS];
static LADSPA_Descriptor descriptor;
static int descriptor_ready = 0;

static void init_descriptor(void) {
  if (descriptor_ready) return;

  port_descriptors[L_INPUT_L] = LADSPA_PORT_INPUT | LADSPA_PORT_AUDIO;
  port_descriptors[L_INPUT_R] = LADSPA_PORT_INPUT | LADSPA_PORT_AUDIO;
  port_descriptors[L_OUTPUT_L] = LADSPA_PORT_OUTPUT | LADSPA_PORT_AUDIO;
  port_descriptors[L_OUTPUT_R] = LADSPA_PORT_OUTPUT | LADSPA_PORT_AUDIO;
  for (int i = L_ENABLE; i < L_N_PORTS; i++)
    port_descriptors[i] = LADSPA_PORT_INPUT | LADSPA_PORT_CONTROL;

  port_names[L_INPUT_L] = "Input L";
  port_names[L_INPUT_R] = "Input R";
  port_names[L_OUTPUT_L] = "Output L";
  port_names[L_OUTPUT_R] = "Output R";
  port_names[L_ENABLE] = "Enable";
  port_names[L_THRESHOLD] = "Threshold (dB)";
  port_names[L_HYST] = "Hysteresis (dB)";
  port_names[L_ATTACK] = "Attack (ms)";
  port_names[L_HOLD] = "Hold (ms)";
  port_names[L_RELEASE] = "Release (ms)";
  port_names[L_RANGE] = "Range (dB)";
  port_names[L_MIX] = "Mix";
  port_names[L_BYPASS] = "Bypass";

  memset(port_hints, 0, sizeof(port_hints));
  for (int i = L_ENABLE; i < L_N_PORTS; i++)
    port_hints[i].HintDescriptor =
      LADSPA_HINT_BOUNDED_BELOW | LADSPA_HINT_BOUNDED_ABOVE | LADSPA_HINT_DEFAULT_MIDDLE;

  port_hints[L_ENABLE].LowerBound = 0.0f;
  port_hints[L_ENABLE].UpperBound = 1.0f;
  port_hints[L_ENABLE].HintDescriptor =
    LADSPA_HINT_BOUNDED_BELOW | LADSPA_HINT_BOUNDED_ABOVE |
    LADSPA_HINT_TOGGLED | LADSPA_HINT_DEFAULT_1;

  port_hints[L_THRESHOLD].LowerBound = -140.0f;
  port_hints[L_THRESHOLD].UpperBound = 0.0f;
  port_hints[L_THRESHOLD].HintDescriptor =
    LADSPA_HINT_BOUNDED_BELOW | LADSPA_HINT_BOUNDED_ABOVE | LADSPA_HINT_DEFAULT_LOW;

  port_hints[L_HYST].LowerBound = 0.0f;
  port_hints[L_HYST].UpperBound = 24.0f;
  port_hints[L_ATTACK].LowerBound = 0.1f;
  port_hints[L_ATTACK].UpperBound = 50.0f;
  port_hints[L_HOLD].LowerBound = 0.0f;
  port_hints[L_HOLD].UpperBound = 500.0f;
  port_hints[L_RELEASE].LowerBound = 1.0f;
  port_hints[L_RELEASE].UpperBound = 1000.0f;
  port_hints[L_RANGE].LowerBound = 0.0f;
  port_hints[L_RANGE].UpperBound = 140.0f;
  port_hints[L_MIX].LowerBound = 0.0f;
  port_hints[L_MIX].UpperBound = 1.0f;
  port_hints[L_MIX].HintDescriptor =
    LADSPA_HINT_BOUNDED_BELOW | LADSPA_HINT_BOUNDED_ABOVE | LADSPA_HINT_DEFAULT_MAXIMUM;

  port_hints[L_BYPASS].LowerBound = 0.0f;
  port_hints[L_BYPASS].UpperBound = 1.0f;
  port_hints[L_BYPASS].HintDescriptor =
    LADSPA_HINT_BOUNDED_BELOW | LADSPA_HINT_BOUNDED_ABOVE |
    LADSPA_HINT_TOGGLED | LADSPA_HINT_DEFAULT_0;

  descriptor.UniqueID = BUSCHAIN_GATE_LADSPA_UNIQUE_ID;
  descriptor.Label = "buschain_gate";
  descriptor.Properties = LADSPA_PROPERTY_HARD_RT_CAPABLE;
  descriptor.Name = "BusChain Gate";
  descriptor.Maker = "BusChain Control";
  descriptor.Copyright = "MIT";
  descriptor.PortCount = L_N_PORTS;
  descriptor.PortDescriptors = port_descriptors;
  descriptor.PortNames = port_names;
  descriptor.PortRangeHints = port_hints;
  descriptor.instantiate = instantiate;
  descriptor.connect_port = connect_port;
  descriptor.activate = activate;
  descriptor.run = run;
  descriptor.run_adding = NULL;
  descriptor.set_run_adding_gain = NULL;
  descriptor.deactivate = NULL;
  descriptor.cleanup = cleanup;

  descriptor_ready = 1;
}

const LADSPA_Descriptor *ladspa_descriptor(unsigned long index) {
  init_descriptor();
  return index == 0 ? &descriptor : NULL;
}
