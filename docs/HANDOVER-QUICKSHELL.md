# Quant handover — Quickshell mixer + scroll strip

BusChain Control owns the **daemon, IPC, and ctl**. Your rice flake (Quant)
owns **Quickshell styling and PanelWindows**. This doc is the contract.

Stubs (unstyled): [`packaging/quickshell/`](../packaging/quickshell/).

---

## Architecture

```
Waybar pill / tray left-click / buschain-ctl popup
        │
        ▼
daemon popup_playback ──► popup_launch
                            │
              ┌─────────────┼─────────────┐
              ▼             ▼             ▼
         Quickshell      GTK mixer     egui popup
         (Quant /        (general      (last resort)
          opted in)       desktop
                          layer-shell)

QS scroll strip ──poke──► adjust_hw_volume ──► RTMIN+9 → Waybar pill
```

---

## Enable

```bash
# Prefer QS for popup + claim Master HW strip (skip GTK strip spawn)
export BUSCHAIN_CONTROL_QS_MIXER=1
export BUSCHAIN_CONTROL_QS_STRIP=1

# Install toggle bridge (tray looks here first)
mkdir -p ~/.config/quickshell/scripts
cp packaging/quickshell/qs-mixer-toggle.sh ~/.config/quickshell/scripts/
chmod +x ~/.config/quickshell/scripts/qs-mixer-toggle.sh
```

Popup order (`popup_launch`):

1. Quickshell — if `BUSCHAIN_CONTROL_QS_MIXER=1`, **or** toggle script exists, **or** `qs` on PATH
2. GTK layer-shell — when `buschain-mixer-gtk` is available (opt out: `USE_GTK_MIXER=0`)
3. egui — `buschain-control --popup` (last resort)

Strip: GTK strip is **opt-in** (`BUSCHAIN_CONTROL_SCROLL_STRIP=1`). Not spawned when
`BUSCHAIN_CONTROL_QS_STRIP=1` or `BUSCHAIN_CONTROL_QS_MIXER=1` (QS owns the hit target).

---

## Socket + ctl

| Item | Value |
|------|--------|
| Socket | `$XDG_RUNTIME_DIR/buschain-control/daemon.sock` |
| Protocol | Newline JSON, `{"op":"…"}` |
| CLI | `buschain-ctl` |

### Poll (source of truth for the panel)

```bash
buschain-ctl mixer
# IPC: {"op":"get_mixer"} → Response type "mixer"
```

### Wake file (optional)

After HW / app / track / device vol+mute mutations, daemon touches:

`$XDG_RUNTIME_DIR/buschain-control/mixer.tick`

QS can `FileView` this instead of blind polling. While panel open, poll ~100–250 ms; when hidden, idle or tick-only.

---

## Mixer JSON schema (`buschain-ctl mixer`)

```json
{
  "status": {
    "master_hw": "alsa_output.…",
    "master_hw_desc": "…",
    "hw_volume_pct": 42,
    "hw_mute": false,
    "session_name": "…",
    "session_slug": "…",
    "sample_rate": 48000,
    "quantum": 256
  },
  "streams": [
    {
      "index": 12,
      "name": "Firefox",
      "meta": "buschain_track_…",
      "volume_pct": 80,
      "mute": false,
      "icon_name": "…",
      "binary": "firefox",
      "app_id": "…",
      "application": "Firefox"
    }
  ],
  "tracks": [
    {
      "id": "uuid",
      "name": "…",
      "kind": "master|track",
      "gain_db": 0.0,
      "mute": false,
      "bus": "buschain_master|buschain_track_…",
      "virtual_output": false,
      "virtual_input": false,
      "virtual_input_source": null
    }
  ],
  "sinks": [
    {
      "name": "…",
      "desc": "…",
      "volume_pct": 50,
      "mute": false,
      "is_master": true,
      "is_default": false,
      "is_virtual": false
    }
  ],
  "sources": [
    {
      "name": "…",
      "desc": "…",
      "volume_pct": 50,
      "mute": false,
      "is_default": true,
      "is_virtual": false
    }
  ],
  "default_sink": "…",
  "default_source": "…"
}
```

Notes:

- Master HW UI is **0–100%** (never boost).
- Track gain is **dB** (−48…+12). GTK mapped UI 0–150 ↔ dB via `20*log10(ui/100)`.
- `Default` sink ≠ `Master HW` (`is_default` vs `is_master`).
- **Virtual devices (output):** `sinks[]` only includes BusChain buses that are
  **app-facing**: `buschain_master` and tracks with channel-rack
  **Create system virtual output** on (`tracks[].virtual_output == true`). Other
  `buschain_track_*` sinks exist for internal routing and are **omitted**. Use
  `is_virtual: true` (or `tracks[].virtual_output`) — do **not** treat every track
  as a device.
- **Virtual devices (input):** `sources[]` may include `buschain_vin_*` when a
  track has **Create system virtual input** on (`tracks[].virtual_input == true`).
  Those are post-FX capture sources (`is_virtual: true`). Internal feed sinks
  (`buschain_vinf_*`) are never listed. See also `tracks[].virtual_input_source`.

---

## Mutations (`buschain-ctl`)

| Action | Command |
|--------|---------|
| HW get / set / scroll | `hw-vol get\|set <pct>\|up [n]\|down [n]` |
| HW mute | `hw-vol mute on\|off\|toggle` |
| App stream vol/mute | `playback vol <index> <pct>` / `playback mute <index> on\|off\|toggle` |
| App → bus/sink | `playback move <index> <sink-or-bus>` |
| Track gain/mute | `track vol <uuid> <db>` / `track mute <uuid> on\|off\|toggle` |
| Default sink/source | `default sink\|source <name>` |
| Master HW device | `master-hw set <sink-name>` |
| Named sink/source | `sink vol\|mute …` / `source vol\|mute …` |
| Open/toggle panel | `popup playback` (or tray / waybar click) |

**Scroll strip:** use `hw-vol up|down` (poke, no reply wait) or raw socket:

```json
{"op":"adjust_hw_volume","delta":5}
{"op":"popup_playback"}
```

Cap **max 2 notches per wheel event**. Notch size = 5%.

---

## Favorites (`mixer-pins.json`)

Path: `~/.config/buschain-control/mixer-pins.json`

Schema: JSON array of track UUID strings (same as GTK / egui).

```json
["aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"]
```

QS reads/writes this file; daemon does not own pins.

---

## Scroll strip geometry

| Env | Default | Meaning |
|-----|---------|---------|
| `BUSCHAIN_CONTROL_SCROLL_ANCHOR` | `left` | `left` or `right` |
| `BUSCHAIN_CONTROL_SCROLL_MARGIN_TOP` | `0` | px from top |
| `BUSCHAIN_CONTROL_SCROLL_MARGIN_X` | `8` | px from left/right |
| `BUSCHAIN_CONTROL_SCROLL_WIDTH` | `110` | hit width |
| `BUSCHAIN_CONTROL_SCROLL_HEIGHT` | `38` | hit height |

Layer-shell: `exclusiveZone = -1` so Hyprland does not push the strip below Waybar.

---

## Waybar

Keep [`packaging/waybar/module.jsonc`](../packaging/waybar/module.jsonc):

- `exec: buschain-ctl status`
- `signal: 9` (daemon RTMIN+9 after HW changes)
- `on-click: buschain-waybar popup`
- **No** `on-scroll-*`

---

## QS IPC

| Call | Behavior |
|------|----------|
| `qs ipc call mixer toggle` | Open if closed / close if open |
| Toggle script | `~/.config/quickshell/scripts/qs-mixer-toggle.sh` — exit 0 = handled |

Optional marker: `$XDG_RUNTIME_DIR/buschain-control/mixer.open` (script flips this if `qs ipc` fails).

---

## Feature parity checklist (before dropping GTK from PATH)

**Mixer panel**

- [ ] Tabs: Playback · Tracks · Output · Input
- [ ] Master HW 0–100% + mute; relative scroll notches; absolute drag set
- [ ] Favorited tracks + app streams on Playback; star ↔ `mixer-pins.json`
- [ ] Tracks: all session tracks, dB gain, mute toggle
- [ ] Output/Input: sinks/sources; Default ≠ Master HW
- [ ] Esc / click-away close; tray toggle closes if open
- [ ] Poll `buschain-ctl mixer` (or tick wake)

**Scroll strip**

- [ ] Transparent hit target over Waybar pill (geometry env)
- [ ] SMOOTH delta → N× ±5% (max 2/event)
- [ ] Click → `popup playback`

**Integration**

- [ ] `BUSCHAIN_CONTROL_QS_MIXER=1` + `QS_STRIP=1`
- [ ] Toggle script installed
- [ ] Waybar has no on-scroll
- [ ] GTK still works if QS unset (fallback)

---

## What Quant does **not** need from BusChain

- Themed QML (rice-side)
- Hyprland `exec-once` for `qs` (rice-side)
- Live peak meters / FX editing in the shell panel
- Removing GTK packages (keep as fallback)
