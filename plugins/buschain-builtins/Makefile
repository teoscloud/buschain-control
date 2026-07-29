PREFIX    ?= $(HOME)/.local
LADSPADIR ?= $(PREFIX)/lib/ladspa
CC        ?= gcc
CFLAGS    ?= -O3 -ffast-math -fPIC -Wall -Wextra -std=c11
LIBS      ?= -lm
BUILD     := build
SO        := $(BUILD)/buschain_builtins.so

.PHONY: all clean install

all: $(SO)

$(BUILD):
	mkdir -p $(BUILD)

$(SO): src/buschain_builtins.c src/ladspa.h | $(BUILD)
	$(CC) $(CFLAGS) -Isrc -shared -o $@ src/buschain_builtins.c $(LIBS)

install: all
	mkdir -p $(LADSPADIR)
	cp -f $(SO) $(LADSPADIR)/

clean:
	rm -rf $(BUILD)
