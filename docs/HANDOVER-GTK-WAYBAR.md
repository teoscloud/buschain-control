# Handover — GTK mixer + Waybar helper

**Date:** 2026-07-29 (revised)  
**Rule:** BusChain owns binaries + popup behavior. **Users own waybar config/CSS** in their rice / Home Manager waybar files. No host flake should auto-install waybar module drop-ins.

---

## Ownership

| Piece | Owner | Location |
|-------|--------|----------|
| `buschain-waybar` helper | Project | `packaging/waybar/buschain-waybar` |
| Example module + CSS | Project (docs only) | `packaging/waybar/module.jsonc`, `style.css` + README |
| GTK panel | Project | `packaging/mixer/legacy/`, `packaging/mixer/buschain-mixer-gtk`, `packaging/nix/gtk-mixer.nix` |
| Popup router | Project | `app/src/popup_launch.rs` |
| Waybar module entry + pill styles | **User rice** | Their `~/.config/waybar/config` + `style.css` |

Copy-paste guide (module, CSS, HM, Hyprland, troubleshooting): project **README → Waybar + mixer popup**.

---

## Waybar popup contract

`on-click: buschain-waybar popup` must open a mixer panel whenever the tray app is running.

### Happy path (recommended)

1. Waybar runs `buschain-waybar popup` (PATH or absolute path to this repo’s helper).
2. Helper finds `buschain-ctl` (`BUSCHAIN_CONTROL_CTL` / `target/debug` / PATH).
3. Helper calls `buschain-ctl popup` against `$XDG_RUNTIME_DIR/buschain-control/daemon.sock`.
4. Tray process runs `spawn_mixer_popup()`:

   1. Quickshell — only if `BUSCHAIN_CONTROL_QS_MIXER=1`
   2. GTK — `buschain-mixer-gtk` / `BUSCHAIN_CONTROL_MIXER` (opt out: `=0`)
   3. egui — `buschain-control --popup`

Tray-owned routing matters: Hyprland/waybar often have a bare PATH without nix-develop libs; the tray started via `cargo run` / packaged wrap has the right environment.

### Offline fallback (no socket / no ctl)

Helper tries checkout/PATH GTK, then checkout/PATH egui `--popup`.

### Dev wiring (`nix develop` + `cargo run`)

| Variable / PATH entry | Purpose |
|----------------------|---------|
| `packaging/waybar` on PATH | `buschain-waybar` without install |
| **`buschain-mixer-gtk` from flake `gtkMixer`** | nix-wrapped panel (gi + layer-shell) — **required** |
| `BUSCHAIN_CONTROL_CTL` | `target/debug/buschain-ctl` |
| `BUSCHAIN_CONTROL_MIXER` | `$(command -v buschain-mixer-gtk)` from the shell |
| `BUSCHAIN_CONTROL_USE_GTK_MIXER=1` | allow GTK auto-detect |

Do **not** point `BUSCHAIN_CONTROL_MIXER` at `packaging/mixer/buschain-mixer-gtk` for
Hyprland/waybar clicks unless that launcher uses a python with `gi` — system
python usually fails with `ModuleNotFoundError: gi`, and a dying child must not
block the egui fallback (`spawn_alive` in `popup_launch.rs`).

### Packaged install (`nix build` / home module)

- `$out/bin/buschain-waybar` wrapped with `PATH` → ctl + `buschain-mixer-gtk`
- Home module puts bins on PATH only — **does not** write waybar config/CSS

### Rice checklist

1. `custom/buschain-control` in waybar modules (config owned by rice).
2. `exec` / `on-click` / scroll → `buschain-waybar` (on PATH **or** absolute path to `packaging/waybar/buschain-waybar`).
3. CSS classes: `online` · `muted` · `offline` (see `packaging/waybar/style.css`).
4. Tray running: `buschain-control --hidden` / `cargo run -- --hidden`.
5. Click → panel (GTK when available, else egui). Scroll → HW volume.

---

## Popup order

1. Quickshell — only if `BUSCHAIN_CONTROL_QS_MIXER=1`
2. GTK — `buschain-mixer-gtk` on PATH / `BUSCHAIN_CONTROL_MIXER` (opt out: `BUSCHAIN_CONTROL_USE_GTK_MIXER=0`)
3. egui — `buschain-control --popup`

---

## Do not regress

- Do not add a Home Manager option that writes waybar config/CSS for users.
- Do not wire a rice flake input that builds BusChain just to refresh waybar snippets.
- Keep example snippets in `packaging/waybar/` in sync with README → Waybar + mixer popup.
- Do not treat “QS script exists” as QS enabled — only `BUSCHAIN_CONTROL_QS_MIXER=1`.
- Prefer **ctl → tray router** for waybar popup; do not require waybar’s PATH to contain GTK/python.
