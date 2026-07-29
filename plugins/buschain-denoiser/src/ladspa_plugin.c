/* BusChain Denoiser — LADSPA wrapper (best for PipeWire filter-chain) */
#include "dsp.h"
#include "ladspa.h"

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define BUSCHAIN_DN_LADSPA_UNIQUE_ID 392001

enum {
  L_INPUT_L = 0,
  L_INPUT_R,
  L_OUTPUT_L,
  L_OUTPUT_R,
  L_THRESHOLD,
  L_RANGE0,
  L_RANGE1,
  L_RANGE2,
  L_RANGE3,
  L_RANGE4,
  L_RANGE5,
  L_HZ0,
  L_HZ1,
  L_HZ2,
  L_HZ3,
  L_HZ4,
  L_HZ5,
  L_HF_BIAS,
  L_STEREO_LINK,
  L_BYPASS,
  L_N_PORTS
};

typedef struct {
  const LADSPA_Data *ports[L_N_PORTS];
  LADSPA_Data *outs[L_N_PORTS];
  BuschainDnState *dsp;
  unsigned long sr;
} BuschainDnLADSPA;

static LADSPA_Handle instantiate(const LADSPA_Descriptor *desc, unsigned long sample_rate) {
  (void)desc;
  BuschainDnLADSPA *self = (BuschainDnLADSPA *)calloc(1, sizeof(BuschainDnLADSPA));
  if (!self) return NULL;
  self->dsp = buschain_dn_create();
  if (!self->dsp) {
    free(self);
    return NULL;
  }
  self->sr = sample_rate;
  buschain_dn_reset(self->dsp, (double)sample_rate);
  return self;
}

static void connect_port(LADSPA_Handle instance, unsigned long port, LADSPA_Data *data) {
  BuschainDnLADSPA *self = (BuschainDnLADSPA *)instance;
  if (port >= L_N_PORTS) return;
  self->ports[port] = data;
  self->outs[port] = data;
}

static void activate(LADSPA_Handle instance) {
  BuschainDnLADSPA *self = (BuschainDnLADSPA *)instance;
  buschain_dn_reset(self->dsp, (double)self->sr);
}

static float pget(BuschainDnLADSPA *self, int p, float def) {
  return self->ports[p] ? (float)(*self->ports[p]) : def;
}

static void run(LADSPA_Handle instance, unsigned long n_samples) {
  BuschainDnLADSPA *self = (BuschainDnLADSPA *)instance;
  BuschainDnParams params;
  buschain_dn_default_params(&params);

  params.threshold_db = pget(self, L_THRESHOLD, params.threshold_db);
  for (int i = 0; i < BUSCHAIN_DN_BANDS; i++) {
    params.range_db[i] = pget(self, L_RANGE0 + i, params.range_db[i]);
    params.center_hz[i] = pget(self, L_HZ0 + i, params.center_hz[i]);
  }
  params.freq_low_hz = 0.0f;
  params.freq_high_hz = 0.0f;
  params.hf_bias = pget(self, L_HF_BIAS, params.hf_bias);
  params.stereo_link = pget(self, L_STEREO_LINK, params.stereo_link);
  params.bypass = pget(self, L_BYPASS, 0.0f) >= 0.5f;

  buschain_dn_set_params(self->dsp, &params);

  const float *in_l = self->ports[L_INPUT_L];
  const float *in_r = self->ports[L_INPUT_R];
  float *out_l = self->outs[L_OUTPUT_L];
  float *out_r = self->outs[L_OUTPUT_R];
  if (!in_l || !out_l) return;
  if (!in_r) in_r = in_l;
  if (!out_r) out_r = out_l;

  buschain_dn_process(self->dsp, in_l, in_r, out_l, out_r, (uint32_t)n_samples);
}

static void cleanup(LADSPA_Handle instance) {
  BuschainDnLADSPA *self = (BuschainDnLADSPA *)instance;
  if (self) {
    buschain_dn_destroy(self->dsp);
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
  for (int i = L_THRESHOLD; i < L_N_PORTS; i++)
    port_descriptors[i] = LADSPA_PORT_INPUT | LADSPA_PORT_CONTROL;

  port_names[L_INPUT_L] = "Input L";
  port_names[L_INPUT_R] = "Input R";
  port_names[L_OUTPUT_L] = "Output L";
  port_names[L_OUTPUT_R] = "Output R";
  port_names[L_THRESHOLD] = "Threshold (dB)";
  port_names[L_RANGE0] = "Range Band 1 (dB)";
  port_names[L_RANGE1] = "Range Band 2 (dB)";
  port_names[L_RANGE2] = "Range Band 3 (dB)";
  port_names[L_RANGE3] = "Range Band 4 (dB)";
  port_names[L_RANGE4] = "Range Band 5 (dB)";
  port_names[L_RANGE5] = "Range Band 6 (dB)";
  port_names[L_HZ0] = "Band 1 Freq (Hz)";
  port_names[L_HZ1] = "Band 2 Freq (Hz)";
  port_names[L_HZ2] = "Band 3 Freq (Hz)";
  port_names[L_HZ3] = "Band 4 Freq (Hz)";
  port_names[L_HZ4] = "Band 5 Freq (Hz)";
  port_names[L_HZ5] = "Band 6 Freq (Hz)";
  port_names[L_HF_BIAS] = "HF Bias";
  port_names[L_STEREO_LINK] = "Stereo Link";
  port_names[L_BYPASS] = "Bypass";

  memset(port_hints, 0, sizeof(port_hints));
  for (int i = L_THRESHOLD; i < L_N_PORTS; i++)
    port_hints[i].HintDescriptor = LADSPA_HINT_BOUNDED_BELOW | LADSPA_HINT_BOUNDED_ABOVE | LADSPA_HINT_DEFAULT_MIDDLE;

  port_hints[L_THRESHOLD].LowerBound = -140.0f;
  port_hints[L_THRESHOLD].UpperBound = 0.0f;
  port_hints[L_THRESHOLD].HintDescriptor =
    LADSPA_HINT_BOUNDED_BELOW | LADSPA_HINT_BOUNDED_ABOVE | LADSPA_HINT_DEFAULT_LOW;

  for (int i = L_RANGE0; i <= L_RANGE5; i++) {
    port_hints[i].LowerBound = 0.0f;
    port_hints[i].UpperBound = 24.0f;
  }
  for (int i = L_HZ0; i <= L_HZ5; i++) {
    port_hints[i].LowerBound = 20.0f;
    port_hints[i].UpperBound = 20000.0f;
    port_hints[i].HintDescriptor =
      LADSPA_HINT_BOUNDED_BELOW | LADSPA_HINT_BOUNDED_ABOVE | LADSPA_HINT_LOGARITHMIC;
  }

  port_hints[L_HF_BIAS].LowerBound = 0.0f;
  port_hints[L_HF_BIAS].UpperBound = 1.0f;
  port_hints[L_STEREO_LINK].LowerBound = 0.0f;
  port_hints[L_STEREO_LINK].UpperBound = 1.0f;
  port_hints[L_BYPASS].LowerBound = 0.0f;
  port_hints[L_BYPASS].UpperBound = 1.0f;
  port_hints[L_BYPASS].HintDescriptor =
    LADSPA_HINT_BOUNDED_BELOW | LADSPA_HINT_BOUNDED_ABOVE |
    LADSPA_HINT_TOGGLED | LADSPA_HINT_DEFAULT_0;

  descriptor.UniqueID = BUSCHAIN_DN_LADSPA_UNIQUE_ID;
  descriptor.Label = "buschain_denoiser";
  descriptor.Properties = LADSPA_PROPERTY_HARD_RT_CAPABLE;
  descriptor.Name = "BusChain Denoiser";
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
