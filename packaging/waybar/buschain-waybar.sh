#!/usr/bin/env bash
# Waybar Master HW volume helper.
#
# Waybar blocks the next scroll tick until on-scroll exits. Calling
# buschain-ctl (Rust cold-start + IPC) on that path feels laggy compared to
# the in-panel HW slider. Scroll hot path = pactl only; daemon sync is background.
set -euo pipefail

CTL="${BUSCHAIN_CONTROL_CTL:-buschain-ctl}"
RUNTIME="${XDG_RUNTIME_DIR:-/tmp}/buschain-control"
CACHE="$RUNTIME/waybar.json"
STATE="$RUNTIME/waybar.state"
LOCK="$RUNTIME/waybar-vol.lock"
mkdir -p "$RUNTIME"

OFFLINE='{"text":"vol —","tooltip":"buschain-control not running (start tray app)","class":"offline"}'

step_once() {
  # Master HW: 5% steps, snap through 100, hard-capped at 100% (no boost).
  local cur="${1:-0}" dir="$2"
  ((cur < 0)) && cur=0
  ((cur > 100)) && cur=100
  local nxt
  if ((dir > 0)); then
    if ((cur >= 100)); then
      nxt=100
    else
      nxt=$(((cur / 5 + 1) * 5))
      ((nxt > 100)) && nxt=100
    fi
  else
    if ((cur <= 0)); then
      nxt=0
    elif ((cur == 100)); then
      nxt=95
    else
      if ((cur % 5 == 0)); then nxt=$((cur - 5)); else nxt=$(((cur / 5) * 5)); fi
      ((nxt < 0)) && nxt=0
    fi
  fi
  echo "$nxt"
}

load_state() {
  SINK=""
  PCT=0
  MUTE=0
  DESC="Master HW"
  # shellcheck disable=SC1090
  [[ -f "$STATE" ]] && source "$STATE" || true
}

write_state() {
  {
    printf 'SINK=%q\n' "$SINK"
    printf 'PCT=%q\n' "$PCT"
    printf 'MUTE=%q\n' "$MUTE"
    printf 'DESC=%q\n' "$DESC"
  } >"$STATE"
}

clamp_hw_pct() {
  local p="${1:-0}"
  ((p < 0)) && p=0
  ((p > 100)) && p=100
  echo "$p"
}

# If Pulse reports boost (>100%), pull Master HW back to 100%.
enforce_hw_cap() {
  [[ -n "${SINK:-}" ]] || return 0
  local live
  live="$(pactl get-sink-volume "$SINK" 2>/dev/null | grep -oE '[0-9]+%' | head -1 | tr -d '%' || true)"
  [[ -n "${live:-}" ]] || return 0
  if ((live > 100)); then
    pactl set-sink-volume "$SINK" "100%" >/dev/null 2>&1 || true
    PCT=100
  else
    PCT="$(clamp_hw_pct "$live")"
  fi
}

write_cache() {
  PCT="$(clamp_hw_pct "${PCT:-0}")"
  local icon="󰕾" class="online"
  if ((MUTE != 0)); then
    icon="󰖁"
    class="muted"
  fi
  local tip
  tip="${DESC} · ${PCT}%"
  printf '{"text":"%s %s%%","tooltip":"%s","percentage":%s,"class":"%s","sink":"%s","muted":%s}\n' \
    "$icon" "$PCT" "$tip" "$PCT" "$class" "$SINK" \
    "$( ((MUTE != 0)) && echo true || echo false )" >"$CACHE"
}

json_field() {
  # tiny extractor: json_field '{"a":"b"}' a  -> b
  local json="$1" key="$2"
  printf '%s' "$json" | sed -n "s/.*\"${key}\":\"\\([^\"]*\\)\".*/\\1/p" | head -1
}

refresh_from_daemon() {
  local json
  json="$("$CTL" status 2>/dev/null || true)"
  [[ -n "$json" ]] || return 1

  local text pct sink
  text="$(json_field "$json" text)"
  pct="$(printf '%s' "$text" | grep -oE '[0-9]+%' | head -1 | tr -d '%' || true)"
  [[ -n "${pct:-}" ]] || pct=0
  PCT="$(clamp_hw_pct "$pct")"
  sink="$(json_field "$json" sink)"
  if [[ -z "$sink" ]]; then
    sink="$(pactl get-default-sink 2>/dev/null || true)"
  fi
  [[ -n "$sink" ]] || sink="@DEFAULT_SINK@"
  SINK="$sink"
  DESC="$(json_field "$json" tooltip | head -1)"
  DESC="${DESC%%$'\n'*}"
  DESC="${DESC%% · *}"
  [[ -n "$DESC" ]] || DESC="Master HW"
  if printf '%s' "$json" | grep -qE '"muted":true|"class":"muted"'; then
    MUTE=1
  else
    MUTE=0
  fi
  enforce_hw_cap
  write_state
  write_cache
  return 0
}

ensure_state() {
  load_state
  if [[ -z "${SINK:-}" ]]; then
    refresh_from_daemon || return 1
    load_state
  fi
  [[ -n "${SINK:-}" ]]
}

nudge() {
  local dir="$1"
  (
    flock 9
    ensure_state || exit 0
    load_state
    # Cheap live read (pactl, not ctl) so mixer drags don't desync steps.
    live="$(pactl get-sink-volume "$SINK" 2>/dev/null | grep -oE '[0-9]+%' | head -1 | tr -d '%' || true)"
    [[ -n "${live:-}" ]] && PCT="$(clamp_hw_pct "$live")"
    # Pull any existing boost back before stepping.
    if [[ -n "${live:-}" ]] && ((live > 100)); then
      pactl set-sink-volume "$SINK" "100%" >/dev/null 2>&1 || true
      PCT=100
    fi
    PCT="$(step_once "$PCT" "$dir")"
    PCT="$(clamp_hw_pct "$PCT")"
    # Instant — same syscall the panel HW slider ends up on.
    pactl set-sink-volume "$SINK" "${PCT}%" >/dev/null 2>&1 || true
    write_state
    write_cache
    # Sync daemon cache off the hot path (do not wait).
    "$CTL" hw-vol set "$PCT" >/dev/null 2>&1 &
  ) 9>"$LOCK"
}

signal_waybar() {
  pkill -RTMIN+9 waybar >/dev/null 2>&1 || true
}

case "${1:-status}" in
  status|display)
    if [[ -f "$CACHE" ]]; then
      now="$(date +%s 2>/dev/null || echo 0)"
      mod="$(stat -c %Y "$CACHE" 2>/dev/null || echo 0)"
      if [[ "$now" =~ ^[0-9]+$ && "$mod" =~ ^[0-9]+$ ]] && ((now - mod < 2)); then
        cat "$CACHE"
        exit 0
      fi
    fi
    if refresh_from_daemon; then
      cat "$CACHE"
    else
      echo "$OFFLINE"
    fi
    ;;
  up)
    nudge 1
    signal_waybar &
    [[ -f "$CACHE" ]] && cat "$CACHE" || true
    ;;
  down)
    nudge -1
    signal_waybar &
    [[ -f "$CACHE" ]] && cat "$CACHE" || true
    ;;
  popup)
    # Prefer Quickshell panel, then egui via ctl/tray launcher (GTK not on hot path).
    if [[ -x "${HOME}/.config/quickshell/scripts/qs-mixer-toggle.sh" ]]; then
      "${HOME}/.config/quickshell/scripts/qs-mixer-toggle.sh" >/dev/null 2>&1 || true
    elif command -v qs >/dev/null 2>&1 && qs ipc call mixer toggle >/dev/null 2>&1; then
      true
    else
      "$CTL" popup playback >/dev/null 2>&1 || true
    fi
    ;;
  *)
    echo "usage: $0 status|up|down|popup" >&2
    exit 2
    ;;
esac
