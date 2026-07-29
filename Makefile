.PHONY: all plugins app run clean install-plugins \
	dev-shell daemon ui ctl mixer rebuild stop stop-restore help

all: plugins

plugins:
	$(MAKE) -C plugins/buschain-denoiser
	$(MAKE) -C plugins/buschain-gate
	$(MAKE) -C plugins/buschain-builtins

run: ui

app:
	cargo build --release -p buschain-control -p buschain-tools

clean:
	$(MAKE) -C plugins/buschain-denoiser clean
	$(MAKE) -C plugins/buschain-gate clean
	$(MAKE) -C plugins/buschain-builtins clean
	cargo clean 2>/dev/null || true
	rm -rf target app/target

install-plugins:
	$(MAKE) -C plugins/buschain-denoiser install
	$(MAKE) -C plugins/buschain-gate install
	$(MAKE) -C plugins/buschain-builtins install

# ── Local vendor workflow (no nixos-rebuild) ──────────────────────────
help:
	@./scripts/dev help

dev-shell:
	./scripts/dev shell

rebuild:
	./scripts/dev rebuild

# Default local path: single cargo run, in-process graph (instant knobs).
daemon:
	./scripts/dev daemon

ui run:
	./scripts/dev run

ctl:
	./scripts/dev ctl -- $(ARGS)

mixer:
	./scripts/dev mixer

stop:
	./scripts/dev stop

stop-restore:
	./scripts/dev stop --restore
