PREFIX        ?= $(HOME)/.local
LIBDIR        ?= $(PREFIX)/lib
LADSPADIR     ?= $(LIBDIR)/ladspa

CC            ?= gcc
CFLAGS        ?= -O3 -ffast-math -fPIC -Wall -Wextra -std=c11
LIBS          ?= -lm

BUILD         := build
LADSPA_SO     := $(BUILD)/buschain_reverb.so

.PHONY: all clean install

all: $(LADSPA_SO)

$(BUILD):
	mkdir -p $(BUILD)

$(LADSPA_SO): src/dsp.c src/ladspa_plugin.c | $(BUILD)
	$(CC) $(CFLAGS) -Isrc -shared -o $@ src/dsp.c src/ladspa_plugin.c $(LIBS)

install: all
	mkdir -p $(LADSPADIR)
	cp -f $(LADSPA_SO) $(LADSPADIR)/

clean:
	rm -rf $(BUILD)
