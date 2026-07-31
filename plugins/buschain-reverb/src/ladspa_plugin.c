/* BusChain Room — LADSPA wrapper */
#include "dsp.h"
#include "ladspa.h"

#include <stdlib.h>
#include <string.h>

#define BUSCHAIN_REVERB_LADSPA_UNIQUE_ID 392017

enum {
  L_INPUT_L = 0,
  L_INPUT_R,
  L_OUTPUT_L,
  L_OUTPUT_R,
  L_BYPASS,
  L_MIX,
  L_PREDELAY,
  L_SIZE,
  L_SHAPE,
  L_RT60,
  L_CHARACTER,
  L_ER_LEVEL,
  L_ER_SPREAD,
  L_DIFFUSION,
  L_DENSITY,
  L_MODULATION,
  L_DECAY_LO,
  L_DECAY_HI,
  L_WET_HP,
  L_WET_LP,
  L_WIDTH,
  L_DUCK_AMOUNT,
  L_DUCK_RELEASE,
  L_FREEZE,
  L_GATE_TIME,
  L_ROOM_TYPE,
  L_SOURCE_X,
  L_SOURCE_Y,
  L_SOURCE_Z,
  L_LISTENER_X,
  L_LISTENER_Y,
  L_LISTENER_Z,
  L_SOURCE_SPACING,
  L_SOURCE_YAW,
  L_FACE_LOCK,
  L_LISTENER_SPACING,
  L_EAR_ANGLE,
  L_EAR_PRESET,
  /* meters (output control) */
  L_M_RT60,
  L_M_ECHO,
  L_M_ER_TAIL,
  L_M_BAND_LO,
  L_M_BAND_MID,
  L_M_BAND_HI,
  L_M_WET_PEAK,
  L_M_DUCK_GR,
  L_N_PORTS
};

typedef struct {
  const LADSPA_Data *ports[L_N_PORTS];
  LADSPA_Data *outs[L_N_PORTS];
  BuschainReverbState *dsp;
  unsigned long sr;
} BuschainReverbLADSPA;

static LADSPA_Handle instantiate(const LADSPA_Descriptor *desc, unsigned long sample_rate) {
  (void)desc;
  BuschainReverbLADSPA *self = (BuschainReverbLADSPA *)calloc(1, sizeof(*self));
  if (!self) return NULL;
  self->dsp = buschain_reverb_create();
  if (!self->dsp) {
    free(self);
    return NULL;
  }
  self->sr = sample_rate;
  buschain_reverb_reset(self->dsp, (double)sample_rate);
  return self;
}

static void connect_port(LADSPA_Handle instance, unsigned long port, LADSPA_Data *data) {
  BuschainReverbLADSPA *self = (BuschainReverbLADSPA *)instance;
  if (port >= L_N_PORTS) return;
  self->ports[port] = data;
  self->outs[port] = data;
}

static void activate(LADSPA_Handle instance) {
  BuschainReverbLADSPA *self = (BuschainReverbLADSPA *)instance;
  buschain_reverb_reset(self->dsp, (double)self->sr);
}

static float pget(BuschainReverbLADSPA *self, int p, float def) {
  return self->ports[p] ? (float)(*self->ports[p]) : def;
}

static void run(LADSPA_Handle instance, unsigned long n_samples) {
  BuschainReverbLADSPA *self = (BuschainReverbLADSPA *)instance;
  BuschainReverbParams params;
  buschain_reverb_default_params(&params);

  params.bypass = pget(self, L_BYPASS, 0.0f) >= 0.5f;
  params.mix = pget(self, L_MIX, params.mix);
  params.predelay_ms = pget(self, L_PREDELAY, params.predelay_ms);
  params.size = pget(self, L_SIZE, params.size);
  params.shape = pget(self, L_SHAPE, params.shape);
  params.rt60 = pget(self, L_RT60, params.rt60);
  params.character = pget(self, L_CHARACTER, params.character);
  params.er_level = pget(self, L_ER_LEVEL, params.er_level);
  params.er_spread = pget(self, L_ER_SPREAD, params.er_spread);
  params.diffusion = pget(self, L_DIFFUSION, params.diffusion);
  params.density = pget(self, L_DENSITY, params.density);
  params.modulation = pget(self, L_MODULATION, params.modulation);
  params.decay_lo = pget(self, L_DECAY_LO, params.decay_lo);
  params.decay_hi = pget(self, L_DECAY_HI, params.decay_hi);
  params.wet_hp_hz = pget(self, L_WET_HP, params.wet_hp_hz);
  params.wet_lp_hz = pget(self, L_WET_LP, params.wet_lp_hz);
  params.width = pget(self, L_WIDTH, params.width);
  params.duck_amount = pget(self, L_DUCK_AMOUNT, params.duck_amount);
  params.duck_release_ms = pget(self, L_DUCK_RELEASE, params.duck_release_ms);
  params.freeze = pget(self, L_FREEZE, 0.0f) >= 0.5f ? 1.0f : 0.0f;
  params.gate_time_ms = pget(self, L_GATE_TIME, params.gate_time_ms);
  params.room_type = pget(self, L_ROOM_TYPE, params.room_type);
  params.source_x = pget(self, L_SOURCE_X, params.source_x);
  params.source_y = pget(self, L_SOURCE_Y, params.source_y);
  params.source_z = pget(self, L_SOURCE_Z, params.source_z);
  params.listener_x = pget(self, L_LISTENER_X, params.listener_x);
  params.listener_y = pget(self, L_LISTENER_Y, params.listener_y);
  params.listener_z = pget(self, L_LISTENER_Z, params.listener_z);
  params.source_spacing = pget(self, L_SOURCE_SPACING, params.source_spacing);
  params.source_yaw = pget(self, L_SOURCE_YAW, params.source_yaw);
  params.face_lock = pget(self, L_FACE_LOCK, params.face_lock) >= 0.5f ? 1.0f : 0.0f;
  params.listener_spacing = pget(self, L_LISTENER_SPACING, params.listener_spacing);
  params.ear_angle = pget(self, L_EAR_ANGLE, params.ear_angle);
  params.ear_preset = pget(self, L_EAR_PRESET, params.ear_preset);

  buschain_reverb_set_params(self->dsp, &params);

  const float *in_l = self->ports[L_INPUT_L];
  const float *in_r = self->ports[L_INPUT_R];
  float *out_l = self->outs[L_OUTPUT_L];
  float *out_r = self->outs[L_OUTPUT_R];
  if (!in_l || !out_l) return;
  if (!in_r) in_r = in_l;
  if (!out_r) out_r = out_l;

  buschain_reverb_process(self->dsp, in_l, in_r, out_l, out_r, (uint32_t)n_samples);

  BuschainReverbMeters m;
  buschain_reverb_get_meters(self->dsp, &m);
  if (self->outs[L_M_RT60]) *self->outs[L_M_RT60] = m.rt60_est;
  if (self->outs[L_M_ECHO]) *self->outs[L_M_ECHO] = m.echo_density;
  if (self->outs[L_M_ER_TAIL]) *self->outs[L_M_ER_TAIL] = m.er_tail;
  if (self->outs[L_M_BAND_LO]) *self->outs[L_M_BAND_LO] = m.band_t60_lo;
  if (self->outs[L_M_BAND_MID]) *self->outs[L_M_BAND_MID] = m.band_t60_mid;
  if (self->outs[L_M_BAND_HI]) *self->outs[L_M_BAND_HI] = m.band_t60_hi;
  if (self->outs[L_M_WET_PEAK]) *self->outs[L_M_WET_PEAK] = m.wet_peak;
  if (self->outs[L_M_DUCK_GR]) *self->outs[L_M_DUCK_GR] = m.duck_gr;
}

static void cleanup(LADSPA_Handle instance) {
  BuschainReverbLADSPA *self = (BuschainReverbLADSPA *)instance;
  if (self) {
    buschain_reverb_destroy(self->dsp);
    free(self);
  }
}

static LADSPA_PortDescriptor port_descriptors[L_N_PORTS];
static LADSPA_PortRangeHint port_hints[L_N_PORTS];
static const char *port_names[L_N_PORTS];
static LADSPA_Descriptor descriptor;
static int descriptor_ready = 0;

static void hint_range(int p, float lo, float hi, int toggled, int def_code) {
  port_hints[p].LowerBound = lo;
  port_hints[p].UpperBound = hi;
  port_hints[p].HintDescriptor =
      LADSPA_HINT_BOUNDED_BELOW | LADSPA_HINT_BOUNDED_ABOVE | def_code;
  if (toggled) port_hints[p].HintDescriptor |= LADSPA_HINT_TOGGLED;
}

static void init_descriptor(void) {
  if (descriptor_ready) return;
  memset(port_hints, 0, sizeof(port_hints));

  port_descriptors[L_INPUT_L] = LADSPA_PORT_INPUT | LADSPA_PORT_AUDIO;
  port_descriptors[L_INPUT_R] = LADSPA_PORT_INPUT | LADSPA_PORT_AUDIO;
  port_descriptors[L_OUTPUT_L] = LADSPA_PORT_OUTPUT | LADSPA_PORT_AUDIO;
  port_descriptors[L_OUTPUT_R] = LADSPA_PORT_OUTPUT | LADSPA_PORT_AUDIO;
  for (int i = L_BYPASS; i <= L_EAR_PRESET; i++)
    port_descriptors[i] = LADSPA_PORT_INPUT | LADSPA_PORT_CONTROL;
  for (int i = L_M_RT60; i < L_N_PORTS; i++)
    port_descriptors[i] = LADSPA_PORT_OUTPUT | LADSPA_PORT_CONTROL;

  port_names[L_INPUT_L] = "Input L";
  port_names[L_INPUT_R] = "Input R";
  port_names[L_OUTPUT_L] = "Output L";
  port_names[L_OUTPUT_R] = "Output R";
  port_names[L_BYPASS] = "Bypass";
  port_names[L_MIX] = "Mix";
  port_names[L_PREDELAY] = "Predelay (ms)";
  port_names[L_SIZE] = "Size";
  port_names[L_SHAPE] = "Shape";
  port_names[L_RT60] = "RT60 (s)";
  port_names[L_CHARACTER] = "Character";
  port_names[L_ER_LEVEL] = "ER Level";
  port_names[L_ER_SPREAD] = "ER Spread";
  port_names[L_DIFFUSION] = "Diffusion";
  port_names[L_DENSITY] = "Density";
  port_names[L_MODULATION] = "Modulation";
  port_names[L_DECAY_LO] = "Decay Lo";
  port_names[L_DECAY_HI] = "Decay Hi";
  port_names[L_WET_HP] = "Wet HP (Hz)";
  port_names[L_WET_LP] = "Wet LP (Hz)";
  port_names[L_WIDTH] = "Width";
  port_names[L_DUCK_AMOUNT] = "Duck Amount";
  port_names[L_DUCK_RELEASE] = "Duck Release (ms)";
  port_names[L_FREEZE] = "Freeze";
  port_names[L_GATE_TIME] = "Gate Time (ms)";
  port_names[L_ROOM_TYPE] = "Room Type";
  port_names[L_SOURCE_X] = "Source X";
  port_names[L_SOURCE_Y] = "Source Y";
  port_names[L_SOURCE_Z] = "Source Z";
  port_names[L_LISTENER_X] = "Listener X";
  port_names[L_LISTENER_Y] = "Listener Y";
  port_names[L_LISTENER_Z] = "Listener Z";
  port_names[L_SOURCE_SPACING] = "Source Spacing";
  port_names[L_SOURCE_YAW] = "Source Yaw";
  port_names[L_FACE_LOCK] = "Face Lock";
  port_names[L_LISTENER_SPACING] = "Listener Spacing";
  port_names[L_EAR_ANGLE] = "Ear Angle";
  port_names[L_EAR_PRESET] = "Ear Preset";
  port_names[L_M_RT60] = "RT60 Est";
  port_names[L_M_ECHO] = "Echo Density";
  port_names[L_M_ER_TAIL] = "ER/Tail";
  port_names[L_M_BAND_LO] = "Band T60 Lo";
  port_names[L_M_BAND_MID] = "Band T60 Mid";
  port_names[L_M_BAND_HI] = "Band T60 Hi";
  port_names[L_M_WET_PEAK] = "Wet Peak";
  port_names[L_M_DUCK_GR] = "Duck GR";

  hint_range(L_BYPASS, 0, 1, 1, LADSPA_HINT_DEFAULT_0);
  hint_range(L_MIX, 0, 1, 0, LADSPA_HINT_DEFAULT_LOW);
  hint_range(L_PREDELAY, 0, 200, 0, LADSPA_HINT_DEFAULT_LOW);
  hint_range(L_SIZE, 0.1f, 4, 0, LADSPA_HINT_DEFAULT_MIDDLE);
  hint_range(L_SHAPE, 0.5f, 2, 0, LADSPA_HINT_DEFAULT_MIDDLE);
  hint_range(L_RT60, 0.1f, 12, 0, LADSPA_HINT_DEFAULT_MIDDLE);
  hint_range(L_CHARACTER, 0, 1, 0, LADSPA_HINT_DEFAULT_MIDDLE);
  hint_range(L_ER_LEVEL, 0, 1, 0, LADSPA_HINT_DEFAULT_MIDDLE);
  hint_range(L_ER_SPREAD, 0, 1, 0, LADSPA_HINT_DEFAULT_MIDDLE);
  hint_range(L_DIFFUSION, 0, 1, 0, LADSPA_HINT_DEFAULT_MIDDLE);
  hint_range(L_DENSITY, 0, 1, 0, LADSPA_HINT_DEFAULT_MIDDLE);
  hint_range(L_MODULATION, 0, 1, 0, LADSPA_HINT_DEFAULT_LOW);
  hint_range(L_DECAY_LO, 0.25f, 2, 0, LADSPA_HINT_DEFAULT_MIDDLE);
  hint_range(L_DECAY_HI, 0.25f, 2, 0, LADSPA_HINT_DEFAULT_MIDDLE);
  hint_range(L_WET_HP, 20, 500, 0, LADSPA_HINT_DEFAULT_LOW);
  hint_range(L_WET_LP, 2000, 20000, 0, LADSPA_HINT_DEFAULT_HIGH);
  hint_range(L_WIDTH, 0, 1, 0, LADSPA_HINT_DEFAULT_HIGH);
  hint_range(L_DUCK_AMOUNT, 0, 1, 0, LADSPA_HINT_DEFAULT_0);
  hint_range(L_DUCK_RELEASE, 10, 1000, 0, LADSPA_HINT_DEFAULT_MIDDLE);
  hint_range(L_FREEZE, 0, 1, 1, LADSPA_HINT_DEFAULT_0);
  hint_range(L_GATE_TIME, 0, 500, 0, LADSPA_HINT_DEFAULT_0);
  hint_range(L_ROOM_TYPE, 0, 6, 0, LADSPA_HINT_DEFAULT_0);
  hint_range(L_SOURCE_X, 0, 1, 0, LADSPA_HINT_DEFAULT_LOW);
  hint_range(L_SOURCE_Y, 0, 1, 0, LADSPA_HINT_DEFAULT_MIDDLE);
  hint_range(L_SOURCE_Z, 0, 1, 0, LADSPA_HINT_DEFAULT_LOW);
  hint_range(L_LISTENER_X, 0, 1, 0, LADSPA_HINT_DEFAULT_HIGH);
  hint_range(L_LISTENER_Y, 0, 1, 0, LADSPA_HINT_DEFAULT_MIDDLE);
  hint_range(L_LISTENER_Z, 0, 1, 0, LADSPA_HINT_DEFAULT_HIGH);
  hint_range(L_SOURCE_SPACING, 0, 1, 0, LADSPA_HINT_DEFAULT_LOW);
  hint_range(L_SOURCE_YAW, -180, 180, 0, LADSPA_HINT_DEFAULT_0);
  hint_range(L_FACE_LOCK, 0, 1, 1, LADSPA_HINT_DEFAULT_1);
  hint_range(L_LISTENER_SPACING, 0, 1, 0, LADSPA_HINT_DEFAULT_LOW);
  hint_range(L_EAR_ANGLE, 90, 180, 0, LADSPA_HINT_DEFAULT_HIGH);
  hint_range(L_EAR_PRESET, 0, 1, 0, LADSPA_HINT_DEFAULT_0);
  for (int i = L_M_RT60; i < L_N_PORTS; i++)
    hint_range(i, 0, 20, 0, LADSPA_HINT_DEFAULT_0);

  descriptor.UniqueID = BUSCHAIN_REVERB_LADSPA_UNIQUE_ID;
  descriptor.Label = "buschain_reverb";
  descriptor.Properties = LADSPA_PROPERTY_HARD_RT_CAPABLE;
  descriptor.Name = "BusChain Room";
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
