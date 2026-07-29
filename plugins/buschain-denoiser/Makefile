PREFIX        ?= $(HOME)/.local
LIBDIR        ?= $(PREFIX)/lib
LV2DIR        ?= $(LIBDIR)/lv2
LADSPADIR     ?= $(LIBDIR)/ladspa

CC            ?= gcc
CFLAGS        ?= -O3 -ffast-math -fPIC -Wall -Wextra -std=c11
CFLAGS        += $(shell pkg-config --cflags lv2 2>/dev/null)
LIBS          ?= -lm

BUILD         := build
LV2_BUNDLE    := $(BUILD)/buschain_denoiser.lv2
LV2_SO        := $(LV2_BUNDLE)/buschain_denoiser.so
LADSPA_SO     := $(BUILD)/buschain_denoiser.so

.PHONY: all clean install

all: $(LV2_SO) $(LADSPA_SO)

$(BUILD):
	mkdir -p $(BUILD)

$(LV2_BUNDLE): | $(BUILD)
	mkdir -p $(LV2_BUNDLE)
	cp lv2/buschain_denoiser.lv2/manifest.ttl lv2/buschain_denoiser.lv2/buschain_denoiser.ttl $(LV2_BUNDLE)/

$(LV2_SO): src/dsp.c src/lv2_plugin.c $(LV2_BUNDLE)
	$(CC) $(CFLAGS) -shared -o $@ src/dsp.c src/lv2_plugin.c $(LIBS)

$(LADSPA_SO): src/dsp.c src/ladspa_plugin.c | $(BUILD)
	$(CC) $(CFLAGS) -Isrc -shared -o $@ src/dsp.c src/ladspa_plugin.c $(LIBS)

install: all
	mkdir -p $(LV2DIR)/buschain_denoiser.lv2 $(LADSPADIR)
	cp -f $(LV2_BUNDLE)/* $(LV2DIR)/buschain_denoiser.lv2/
	cp -f $(LADSPA_SO) $(LADSPADIR)/

clean:
	rm -rf $(BUILD)
