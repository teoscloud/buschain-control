#!/usr/bin/env python3
"""BusChain Control overlay — modern Playback / Tracks / Output / Input."""

from __future__ import annotations

import atexit
import json
import math
import os
import queue
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
try:
    gi.require_version("GtkLayerShell", "0.1")
    from gi.repository import GtkLayerShell

    HAS_LAYER = True
except (ValueError, ImportError):
    HAS_LAYER = False

from gi.repository import Gdk, GLib, Gtk, Pango  # noqa: E402

RUNTIME = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "buschain-control"
PID_FILE = RUNTIME / "mixer.pid"
CONFIG_DIR = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "buschain-control"
# Favorites (legacy filename kept so existing pins migrate).
FAVORITES_FILE = CONFIG_DIR / "mixer-pins.json"
STYLE_CANDIDATES = [
    Path(os.environ["BUSCHAIN_CONTROL_MIXER_CSS"])
    if os.environ.get("BUSCHAIN_CONTROL_MIXER_CSS")
    else None,
    Path(__file__).resolve().parent / "style.css",
]

CARD_W = 64
DB_MIN, DB_MAX = -48.0, 12.0
VOL_STEP = 5
VOL_MAX = 150.0  # apps + mixer tracks may boost
HW_VOL_MAX = 100.0  # Master HW / device sinks — no boost
VOL_SNAP = 100.0


def resolve_ctl() -> str:
    """Waybar/Hyprland often have a thin PATH — prefer env + checkout bins."""
    env = os.environ.get("BUSCHAIN_CONTROL_CTL")
    if env and Path(env).is_file():
        return env
    home = Path.home()
    for cand in (
        home / "Projects/buschain-control/target/debug/buschain-ctl",
        home / "Projects/buschain-control/target/release/buschain-ctl",
    ):
        if cand.is_file() and os.access(cand, os.X_OK):
            return str(cand)
    return env or "buschain-ctl"


CTL = resolve_ctl()

# One worker — racing Thread-per-call let older `hw-vol set` win over newer notches.
_CTL_Q: queue.Queue[tuple[str, ...] | None] = queue.Queue()
_CTL_WORKER_LOCK = threading.Lock()
_CTL_WORKER_STARTED = False


def _ensure_ctl_worker() -> None:
    global _CTL_WORKER_STARTED
    with _CTL_WORKER_LOCK:
        if _CTL_WORKER_STARTED:
            return
        _CTL_WORKER_STARTED = True

        def _loop() -> None:
            while True:
                item = _CTL_Q.get()
                if item is None:
                    return
                try:
                    subprocess.run(
                        [CTL, *item], capture_output=True, text=True, check=False
                    )
                except FileNotFoundError:
                    pass

        threading.Thread(target=_loop, daemon=True, name="buschain-ctl-q").start()


def _lat(msg: str) -> None:
    if os.environ.get("BUSCHAIN_CONTROL_LAT_TRACE", "").lower() in (
        "1",
        "true",
        "yes",
    ):
        print(f"[buschain-lat] {msg}", file=sys.stderr, flush=True)


def ctl_queue(*args: str) -> None:
    """Ordered async ctl (Master HW scroll/drag)."""
    _ensure_ctl_worker()
    _CTL_Q.put(tuple(args))


def ctl_async(*args: str) -> None:
    # Non-HW paths: still serialised so mute/vol never overtake each other.
    ctl_queue(*args)


def ctl(*args: str) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run([CTL, *args], capture_output=True, text=True, check=False)
    except FileNotFoundError:
        return subprocess.CompletedProcess([CTL, *args], returncode=127, stdout="", stderr="")


def fetch_state() -> dict:
    t0 = time.monotonic()
    r = ctl("devices", "list")
    if r.returncode != 0 or not r.stdout.strip():
        r = ctl("playback", "list")
    _lat(f"fetch_state {int((time.monotonic() - t0) * 1000)}ms rc={r.returncode}")
    if r.returncode != 0 or not r.stdout.strip():
        return {}
    try:
        return json.loads(r.stdout)
    except json.JSONDecodeError:
        return {}


def load_favorites() -> list[str]:
    try:
        data = json.loads(FAVORITES_FILE.read_text())
        if isinstance(data, list):
            return [str(x) for x in data]
    except (OSError, json.JSONDecodeError, TypeError):
        pass
    return []


def save_favorites(favs: list[str]) -> None:
    CONFIG_DIR.mkdir(parents=True, exist_ok=True)
    FAVORITES_FILE.write_text(json.dumps(favs, indent=2) + "\n")


def db_to_ui(db: float) -> float:
    """Map gain_db → 0..150 UI scale (matches bus volume mapping)."""
    lin = 10.0 ** (float(db) / 20.0)
    return max(0.0, min(150.0, lin * 100.0))


def ui_to_db(ui: float) -> float:
    lin = max(1e-4, float(ui) / 100.0)
    return max(DB_MIN, min(DB_MAX, 20.0 * math.log10(lin)))


def _proc_state(pid: int) -> str | None:
    """Return `/proc/<pid>` state char, or None if missing."""
    try:
        text = Path(f"/proc/{pid}/stat").read_text()
    except OSError:
        return None
    # `pid (comm) state ...` — comm may contain spaces/parens.
    rparen = text.rfind(")")
    if rparen < 0 or rparen + 2 >= len(text):
        return None
    return text[rparen + 2]


def _proc_cmdline(pid: int) -> str:
    try:
        raw = Path(f"/proc/{pid}/cmdline").read_bytes()
    except OSError:
        return ""
    return raw.replace(b"\x00", b" ").decode("utf-8", "replace").lower()


def pid_is_live_mixer(pid: int) -> bool:
    """Reject dead PIDs and zombies — `os.kill(pid, 0)` succeeds for zombies."""
    if pid <= 0:
        return False
    state = _proc_state(pid)
    if state is None or state == "Z":
        return False
    cmd = _proc_cmdline(pid)
    if not cmd:
        return False
    return "buschain-mixer" in cmd or "buschain_mixer" in cmd


def already_running() -> int | None:
    if not PID_FILE.exists():
        return None
    try:
        pid = int(PID_FILE.read_text().strip())
    except ValueError:
        PID_FILE.unlink(missing_ok=True)
        return None
    if not pid_is_live_mixer(pid):
        PID_FILE.unlink(missing_ok=True)
        return None
    return pid


def claim_pid() -> None:
    RUNTIME.mkdir(parents=True, exist_ok=True)
    PID_FILE.write_text(str(os.getpid()))

    def _clear() -> None:
        try:
            if PID_FILE.exists() and PID_FILE.read_text().strip() == str(os.getpid()):
                PID_FILE.unlink(missing_ok=True)
        except OSError:
            pass

    atexit.register(_clear)


def load_css() -> None:
    provider = Gtk.CssProvider()
    css_path = next((p for p in STYLE_CANDIDATES if p and p.is_file()), None)
    try:
        if css_path:
            provider.load_from_path(str(css_path))
        else:
            provider.load_from_data(b".panel { background-color: #1a1b1f; border-radius: 20px; }")
    except GLib.Error as err:
        sys.stderr.write(f"buschain-mixer: css load failed: {err}\n")
        provider.load_from_data(
            b".panel { background-color: #1a1b1f; border-radius: 20px; padding: 14px; }"
        )
    screen = Gdk.Screen.get_default()
    # Beat Adwaita / system themes that force square buttons.
    Gtk.StyleContext.add_provider_for_screen(
        screen, provider, Gtk.STYLE_PROVIDER_PRIORITY_USER
    )


def _icon_candidates(stream: dict) -> list[str]:
    out: list[str] = []
    for key in ("icon_name", "app_id", "binary", "application"):
        v = stream.get(key)
        if isinstance(v, str) and v.strip():
            out.append(v.strip())
    extra: list[str] = []
    for c in list(out):
        low = c.lower()
        extra.append(low)
        extra.append(low.replace(" ", "-"))
        if low.endswith(".desktop"):
            extra.append(low[: -len(".desktop")])
        if "brave" in low:
            extra.extend(["brave-browser", "brave"])
        if "chromium" in low or "chrome" in low:
            extra.extend(["chromium", "google-chrome", "chromium-browser"])
        if "firefox" in low:
            extra.append("firefox")
        if "spotify" in low:
            extra.append("spotify")
        if "discord" in low:
            extra.append("discord")
    for e in extra:
        if e not in out:
            out.append(e)
    return out


def _icon_theme() -> Gtk.IconTheme:
    """Prefer Adwaita — WhiteSur-dark aborts when `image-missing` is absent."""
    for name in ("Adwaita", "AdwaitaLegacy", "hicolor"):
        t = Gtk.IconTheme.new()
        try:
            t.set_custom_theme(name)
        except Exception:
            continue
        if t.has_icon("image-missing") or t.has_icon("audio-volume-high-symbolic"):
            return t
    return Gtk.IconTheme.get_default()


_ICON_THEME: Gtk.IconTheme | None = None


def icon_theme() -> Gtk.IconTheme:
    global _ICON_THEME
    if _ICON_THEME is None:
        _ICON_THEME = _icon_theme()
    return _ICON_THEME


def _pixbuf_icon(name: str, size: int = 16) -> Gtk.Image | None:
    """Load via pixbuf only — never `new_from_icon_name` (theme abort on miss)."""
    theme = icon_theme()
    if not theme.has_icon(name):
        return None
    try:
        pix = theme.load_icon(name, size, Gtk.IconLookupFlags.FORCE_SIZE)
        img = Gtk.Image.new_from_pixbuf(pix)
        return img
    except (GLib.Error, Exception):
        return None


def resolve_app_icon(stream: dict, size: int = 28) -> Gtk.Image | None:
    for name in _icon_candidates(stream):
        img = _pixbuf_icon(name, size)
        if img is not None:
            img.get_style_context().add_class("app-icon")
            return img
    img = _pixbuf_icon("audio-volume-high-symbolic", size)
    if img is not None:
        img.get_style_context().add_class("app-icon")
        return img
    return None


def icon_image(name: str, fallback: str = "●") -> Gtk.Widget:
    img = _pixbuf_icon(name, 16)
    if img is not None:
        return img
    return Gtk.Label(label=fallback)


def make_icon_toggle(muted: bool) -> Gtk.ToggleButton:
    btn = Gtk.ToggleButton()
    btn.get_style_context().add_class("icon-btn")
    btn.set_relief(Gtk.ReliefStyle.NONE)
    btn.set_active(muted)
    _set_mute_icon(btn)
    return btn


def _set_mute_icon(btn: Gtk.ToggleButton) -> None:
    muted = btn.get_active()
    name = "audio-volume-muted-symbolic" if muted else "audio-volume-high-symbolic"
    btn.set_image(icon_image(name, "🔇" if muted else "🔊"))
    btn.set_always_show_image(True)
    btn.set_label("")
    ctx = btn.get_style_context()
    if muted:
        ctx.add_class("active")
    else:
        ctx.remove_class("active")


def _set_star_icon(btn: Gtk.ToggleButton) -> None:
    starred = btn.get_active()
    btn.set_image(icon_image("starred-symbolic" if starred else "non-starred-symbolic", "★" if starred else "☆"))
    btn.set_always_show_image(True)
    btn.set_label("")
    ctx = btn.get_style_context()
    if starred:
        ctx.add_class("active")
    else:
        ctx.remove_class("active")


def make_star_toggle(starred: bool) -> Gtk.ToggleButton:
    btn = Gtk.ToggleButton()
    btn.get_style_context().add_class("star-btn")
    btn.set_relief(Gtk.ReliefStyle.NONE)
    btn.set_active(starred)
    _set_star_icon(btn)
    return btn


def _scroll_direction(event) -> int:
    """+1 volume up, -1 volume down, 0 = ignore. Wheel-up raises volume.

    App/stream faders: one notch per event above a small deadzone.
    Master HW uses `_hw_scroll_notches` (accumulator) instead.
    """
    if event.direction == Gdk.ScrollDirection.SMOOTH:
        dx = float(getattr(event, "delta_x", 0.0) or 0.0)
        dy = float(getattr(event, "delta_y", 0.0) or 0.0)
        dead = 0.08
        if abs(dy) >= abs(dx):
            if abs(dy) < dead:
                return 0
            return 1 if dy < 0 else -1
        if abs(dx) < dead:
            return 0
        return 1 if dx < 0 else -1
    if event.direction in (Gdk.ScrollDirection.UP, Gdk.ScrollDirection.LEFT):
        return 1
    if event.direction in (Gdk.ScrollDirection.DOWN, Gdk.ScrollDirection.RIGHT):
        return -1
    return 0


# Per-scale smooth residue for Master HW (libinput sends many tiny deltas).
_HW_SCROLL_ACCUM: dict[int, float] = {}
_HW_SCROLL_EVENT_SEEN: dict[int, int] = {}


def _hw_scroll_notches(scale: Gtk.Scale, event) -> int:
    """Signed notch count for one Gdk scroll on Master HW (0 = none yet)."""
    sid = id(scale)
    et = int(getattr(event, "time", 0) or 0)
    if et and _HW_SCROLL_EVENT_SEEN.get(sid) == et:
        return 0
    if et:
        _HW_SCROLL_EVENT_SEEN[sid] = et

    if event.direction == Gdk.ScrollDirection.SMOOTH:
        dx = float(getattr(event, "delta_x", 0.0) or 0.0)
        dy = float(getattr(event, "delta_y", 0.0) or 0.0)
        # Positive accum = louder (wheel up / negative dy).
        delta = (-dy) if abs(dy) >= abs(dx) else (-dx)
        if abs(delta) < 1e-6:
            return 0
        acc = _HW_SCROLL_ACCUM.get(sid, 0.0) + delta
        notches = int(acc)  # toward zero truncates; ±1.0 → one notch
        if notches == 0:
            _HW_SCROLL_ACCUM[sid] = acc
            return 0
        _HW_SCROLL_ACCUM[sid] = acc - float(notches)
        return notches

    if event.direction in (Gdk.ScrollDirection.UP, Gdk.ScrollDirection.LEFT):
        return 1
    if event.direction in (Gdk.ScrollDirection.DOWN, Gdk.ScrollDirection.RIGHT):
        return -1
    return 0


def snap_step_volume(cur: float, direction: int, vol_max: float = VOL_MAX) -> float:
    """±5 on a 5-grid; land on 100 when crossing it; never exceed vol_max."""
    vmax = float(vol_max)
    cur = max(0.0, min(vmax, float(cur)))
    if direction > 0:
        if cur >= vmax:
            return vmax
        if cur < VOL_SNAP:
            nxt = (int(cur) // VOL_STEP + 1) * VOL_STEP
            return float(min(nxt, int(VOL_SNAP), int(vmax)))
        if cur == VOL_SNAP:
            if vmax <= VOL_SNAP:
                return VOL_SNAP
            return float(min(VOL_SNAP + VOL_STEP, vmax))
        nxt = (int(cur) // VOL_STEP + 1) * VOL_STEP
        return float(min(nxt, int(vmax)))
    if cur <= 0:
        return 0.0
    if cur > VOL_SNAP and vmax > VOL_SNAP:
        if int(cur) % VOL_STEP == 0:
            nxt = int(cur) - VOL_STEP
        else:
            nxt = (int(cur) // VOL_STEP) * VOL_STEP
        return float(max(nxt, int(VOL_SNAP)))
    if cur == VOL_SNAP:
        return VOL_SNAP - VOL_STEP
    if int(cur) % VOL_STEP == 0:
        nxt = int(cur) - VOL_STEP
    else:
        nxt = (int(cur) // VOL_STEP) * VOL_STEP
    return float(max(nxt, 0))


def _scale_under_scroll(scroll: Gtk.ScrolledWindow, event) -> Gtk.Scale | None:
    """Hit-test a Scale inside a ScrolledWindow (accounts for adj offsets)."""
    try:
        x = float(event.x)
        y = float(event.y)
    except Exception:
        return None
    hadj = scroll.get_hadjustment()
    vadj = scroll.get_vadjustment()
    # Event coords are in the visible viewport; child layout is in content space.
    x += float(hadj.get_value()) if hadj is not None else 0.0
    y += float(vadj.get_value()) if vadj is not None else 0.0
    child = scroll.get_child()
    if child is None:
        return None

    def walk(w) -> Gtk.Scale | None:
        if isinstance(w, Gtk.Scale):
            alloc = w.get_allocation()
            try:
                ok, wx, wy = w.translate_coordinates(child, 0, 0)
            except Exception:
                return None
            if not ok:
                return None
            if wx <= x <= wx + alloc.width and wy <= y <= wy + alloc.height:
                return w
            return None
        if hasattr(w, "get_children"):
            for c in w.get_children():
                hit = walk(c)
                if hit is not None:
                    return hit
        # Viewport wraps the real child.
        if hasattr(w, "get_child"):
            inner = w.get_child()
            if inner is not None and inner is not w:
                return walk(inner)
        return None

    return walk(child)


# Dedup only when the same GdkEvent is delivered to Scale + row + ScrolledWindow.
# Do NOT time-throttle — that drops real mouse-wheel notches (~15–40ms apart).
_SCROLL_EVENT_SEEN: dict[int, int] = {}


def _apply_scale_scroll(scale: Gtk.Scale, event, vol_max: float) -> bool:
    direction = _scroll_direction(event)
    if direction == 0:
        return False
    sid = id(scale)
    # GDK event time is ms; identical across widgets for one delivery.
    et = int(getattr(event, "time", 0) or 0)
    if et and _SCROLL_EVENT_SEEN.get(sid) == et:
        return True
    if et:
        _SCROLL_EVENT_SEEN[sid] = et
    scale.set_value(snap_step_volume(scale.get_value(), direction, vol_max=vol_max))
    return True


def _bind_scale_scroll(scale: Gtk.Scale, vol_max: float = VOL_MAX) -> None:
    scale.add_events(Gdk.EventMask.SCROLL_MASK | Gdk.EventMask.SMOOTH_SCROLL_MASK)

    def on_scroll(sc: Gtk.Scale, event) -> bool:
        if _apply_scale_scroll(sc, event, vol_max):
            return True
        # Unused micro-ticks must not be swallowed (allow strip pan / parent).
        return False

    def on_release(sc: Gtk.Scale, _event) -> bool:
        v = sc.get_value()
        if abs(v - VOL_SNAP) <= 4.0 and v != VOL_SNAP:
            sc.set_value(min(VOL_SNAP, vol_max))
        return False

    scale.connect("scroll-event", on_scroll)
    scale.connect("button-release-event", on_release)


def _bind_scroll_on_container(
    container: Gtk.Widget, scale: Gtk.Scale, vol_max: float = VOL_MAX
) -> None:
    """Horizontal Output/HW rows: parent often gets the wheel before the Scale."""
    container.add_events(Gdk.EventMask.SCROLL_MASK | Gdk.EventMask.SMOOTH_SCROLL_MASK)

    def on_scroll(_w, event) -> bool:
        # Always drive the bound scale when hovering its row (don't scroll lists).
        if _apply_scale_scroll(scale, event, vol_max):
            return True
        return True  # consume micro-ticks over the fader row

    container.connect("scroll-event", on_scroll)


def _bind_master_hw_scroll(
    scale: Gtk.Scale,
    container: Gtk.Widget | None,
    on_notches,
) -> None:
    """Single Master HW scroll path — relative notches, no GTK Range default."""
    scale._buschain_hw = True  # type: ignore[attr-defined]
    scale._buschain_hw_notches = on_notches  # type: ignore[attr-defined]
    mask = Gdk.EventMask.SCROLL_MASK | Gdk.EventMask.SMOOTH_SCROLL_MASK
    scale.add_events(mask)

    def handle(_w, event) -> bool:
        n = _hw_scroll_notches(scale, event)
        if n != 0:
            on_notches(n)
        # Always consume — micro-ticks must not pan lists or nudge off-grid.
        return True

    scale.connect("scroll-event", handle)
    if container is not None:
        container.add_events(mask)
        container.connect("scroll-event", handle)


def _bind_hscroll_wheel(scroll: Gtk.ScrolledWindow) -> None:
    """Playback / Tracks strip: pan horizontally when not over a fader."""
    scroll.add_events(Gdk.EventMask.SCROLL_MASK | Gdk.EventMask.SMOOTH_SCROLL_MASK)

    def on_scroll(w, event) -> bool:
        hit = _scale_under_scroll(w, event)
        if hit is not None:
            # Drive the fader here — more reliable than relying on Scale delivery.
            if _apply_scale_scroll(hit, event, VOL_MAX):
                return True
            return True
        hadj = scroll.get_hadjustment()
        if hadj is None:
            return False
        page = float(hadj.get_page_increment() or 0.0)
        step = page * 0.35 if page > 0 else float(hadj.get_step_increment() or 80.0)
        if step < 40.0:
            step = 80.0
        dx = 0.0
        if event.direction == Gdk.ScrollDirection.SMOOTH:
            dx_raw = float(getattr(event, "delta_x", 0.0) or 0.0)
            dy_raw = float(getattr(event, "delta_y", 0.0) or 0.0)
            if abs(dx_raw) >= abs(dy_raw) and abs(dx_raw) >= 0.1:
                dx = dx_raw * step
            elif abs(dx_raw) < 0.1 and abs(dy_raw) >= 0.1:
                dx = dy_raw * step
            else:
                return False
        elif event.direction in (Gdk.ScrollDirection.UP, Gdk.ScrollDirection.LEFT):
            dx = -step
        elif event.direction in (Gdk.ScrollDirection.DOWN, Gdk.ScrollDirection.RIGHT):
            dx = step
        else:
            return False
        upper = hadj.get_upper() - hadj.get_page_size()
        hadj.set_value(max(hadj.get_lower(), min(hadj.get_value() + dx, upper)))
        return True

    scroll.connect("scroll-event", on_scroll)


def _bind_vscroll_yield_to_scale(
    scroll: Gtk.ScrolledWindow, vol_max: float = HW_VOL_MAX
) -> None:
    """Output/Input list: wheel over a volume row adjusts %, else scrolls the list."""
    scroll.add_events(Gdk.EventMask.SCROLL_MASK | Gdk.EventMask.SMOOTH_SCROLL_MASK)

    def on_scroll(w, event) -> bool:
        hit = _scale_under_scroll(w, event)
        if hit is None:
            return False
        # Master HW: relative accumulator path (same as row/scale binders).
        if getattr(hit, "_buschain_hw", False):
            n = _hw_scroll_notches(hit, event)
            cb = getattr(hit, "_buschain_hw_notches", None)
            if n != 0 and callable(cb):
                cb(n)
            return True
        if _apply_scale_scroll(hit, event, vol_max):
            return True
        return True  # over fader: don't pan the device list on micro-ticks

    scroll.connect("scroll-event", on_scroll)


class Mixer(Gtk.Window):
    def __init__(self) -> None:
        super().__init__(title="BusChain Control")
        self.set_decorated(False)
        self.set_resizable(False)
        self.set_keep_above(True)
        self.set_skip_taskbar_hint(True)
        self.set_skip_pager_hint(True)
        # RGBA visual so panel border-radius isn't clipped to a square opaque window.
        self.set_app_paintable(True)
        screen = self.get_screen()
        visual = screen.get_rgba_visual() if screen is not None else None
        if visual is not None:
            self.set_visual(visual)
        self._building = False
        self._interacting = False
        self._interact_timer: int | None = None
        # Keys with an active pointer drag — never patch these from daemon refresh.
        self._drag_keys: set[str] = set()
        # Keys recently changed locally — suppress patch snap-back (ms wall clock).
        self._local_until: dict[str, float] = {}
        self._opened_at = time.monotonic()
        self._play_fp: tuple | None = None
        self._tracks_fp: tuple | None = None
        self._out_fp: tuple | None = None
        self._in_fp: tuple | None = None
        self._stream_widgets: dict[str, dict] = {}
        self._device_widgets: dict[str, dict] = {}
        self._vol_timers: dict[str, int] = {}
        self._favorites = load_favorites()
        self._tracks_cache: list[dict] = []
        self._tick_ms = 400

        if HAS_LAYER:
            GtkLayerShell.init_for_window(self)
            GtkLayerShell.set_layer(self, GtkLayerShell.Layer.OVERLAY)
            GtkLayerShell.set_anchor(self, GtkLayerShell.Edge.TOP, True)
            GtkLayerShell.set_anchor(self, GtkLayerShell.Edge.LEFT, True)
            GtkLayerShell.set_margin(self, GtkLayerShell.Edge.TOP, 46)
            GtkLayerShell.set_margin(self, GtkLayerShell.Edge.LEFT, 12)
            GtkLayerShell.set_namespace(self, "buschain-mixer")
            # Exclusive keyboard so Esc works; click-outside closes via focus-out.
            GtkLayerShell.set_keyboard_mode(
                self, GtkLayerShell.KeyboardMode.EXCLUSIVE
            )

        self.connect("key-press-event", self._on_key)
        self.connect("focus-out-event", self._on_focus_out)
        self.connect("destroy", Gtk.main_quit)

        self.panel = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        self.panel.get_style_context().add_class("panel")
        self.add(self.panel)

        # No title chrome — tabs are enough; Esc / click-outside still close.
        self.nb = Gtk.Notebook()
        self.nb.get_style_context().add_class("tabs")
        self.nb.set_tab_pos(Gtk.PositionType.TOP)
        self.nb.set_show_border(False)
        self.panel.pack_start(self.nb, True, True, 0)

        self.play_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        self.tracks_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        self.out_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        self.in_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        self.nb.append_page(self.play_box, Gtk.Label(label="Playback"))
        self.nb.append_page(self.tracks_box, Gtk.Label(label="Tracks"))
        self.nb.append_page(self.out_box, Gtk.Label(label="Output"))
        self.nb.append_page(self.in_box, Gtk.Label(label="Input"))

        # Fast tick while mapped so waybar / external Master HW changes appear soon.
        GLib.timeout_add(self._tick_ms, self._tick)

    def _hydrate_hw_fast(self) -> None:
        """Cheap Master HW row before full devices list (paint-first)."""
        r = ctl("status")
        if r.returncode != 0 or not r.stdout.strip():
            return
        st: dict = {}
        try:
            raw = json.loads(r.stdout)
            if isinstance(raw, dict) and "percentage" in raw:
                tip = str(raw.get("tooltip") or "")
                st = {
                    "hw_volume_pct": int(raw.get("percentage") or 0),
                    "hw_mute": bool(raw.get("muted")),
                    "master_hw": raw.get("sink") or "",
                    "master_hw_desc": tip.split("\n")[0] if tip else "Master HW",
                }
        except json.JSONDecodeError:
            pass
        if not st:
            g = ctl("hw-vol", "get")
            if g.returncode == 0 and g.stdout.strip().isdigit():
                st = {"hw_volume_pct": int(g.stdout.strip()), "hw_mute": False}
        if not st:
            return
        self._last_status = st
        self._building = True
        try:
            self._rebuild_playback({"status": st, "streams": []})
        finally:
            self._building = False
        self.show_all()

    def _initial_refresh(self) -> bool:
        t0 = time.monotonic()
        try:
            self._hydrate_hw_fast()
        except Exception as e:
            _lat(f"hydrate_hw_fast err {e}")
        self.refresh()
        _lat(f"initial_refresh {int((time.monotonic() - t0) * 1000)}ms")
        return False

    def _on_key(self, _w, event) -> bool:
        if event.keyval == Gdk.KEY_Escape:
            self.close()
            return True
        return False

    def _on_focus_out(self, _w, _event) -> bool:
        # Ignore opening click / layer-shell keyboard-grab churn from waybar.
        # Must stay above tray spawn_alive grace (~450ms) or we die mid-probe.
        if time.monotonic() - self._opened_at < 1.0:
            return False
        # Don't leave drag locks stuck if release was eaten by focus churn.
        self._clear_drag_locks()

        def _close() -> bool:
            if not self.get_window():
                return False
            # Still focused (e.g. transient focus bounce) — keep open.
            if self.has_toplevel_focus():
                return False
            self.close()
            return False

        GLib.timeout_add(120, _close)
        return False

    def _tick(self) -> bool:
        # Only skip while building or mid pointer-drag. Wheel holds use
        # `_is_local` so waybar / other faders still refresh.
        if self._building or self._drag_keys:
            return True
        self.refresh()
        return True

    def _touch_local(self, key: str, hold_ms: int = 900) -> None:
        """Suppress daemon→UI patch for this control so the thumb can't snap back."""
        self._local_until[key] = time.monotonic() + hold_ms / 1000.0

    def _is_local(self, key: str) -> bool:
        if key in self._drag_keys:
            return True
        until = self._local_until.get(key)
        if until is None:
            return False
        if time.monotonic() >= until:
            self._local_until.pop(key, None)
            return False
        return True

    def _clear_drag_locks(self) -> None:
        """Lost button-release (focus-out / grab) must not freeze patch forever."""
        self._drag_keys.clear()
        self._interacting = False

    def _wire_scale_guard(self, scale: Gtk.Scale, key: str) -> None:
        """Hold patch lock for the whole pointer drag, not just 700ms after last move."""

        def on_press(_sc, event) -> bool:
            if event.button == 1:
                self._drag_keys.add(key)
                self._interacting = True
                self._touch_local(key, 1500)
                # Safety: release can be lost under layer-shell focus churn.
                def _safety() -> bool:
                    self._drag_keys.discard(key)
                    return False

                GLib.timeout_add(2500, _safety)
            return False

        def on_release(_sc, event) -> bool:
            if event.button == 1:
                self._drag_keys.discard(key)
                self._touch_local(key, 700)
                self._bump_interact(400)
            return False

        scale.add_events(
            Gdk.EventMask.BUTTON_PRESS_MASK | Gdk.EventMask.BUTTON_RELEASE_MASK
        )
        scale.connect("button-press-event", on_press)
        scale.connect("button-release-event", on_release)

    def _sync_hw_ui(self, pct: int, mute: bool | None = None) -> None:
        """Keep Playback Master HW + Output master row + status cache aligned."""
        pct = min(max(int(pct), 0), int(HW_VOL_MAX))
        st = getattr(self, "_last_status", None)
        if not isinstance(st, dict):
            st = {}
            self._last_status = st
        st["hw_volume_pct"] = pct
        if mute is not None:
            st["hw_mute"] = bool(mute)
        self._building = True
        try:
            if hasattr(self, "_hw_scale"):
                if int(round(self._hw_scale.get_value())) != pct:
                    self._hw_scale.set_value(pct)
            if mute is not None and hasattr(self, "_hw_mute"):
                if bool(self._hw_mute.get_active()) != bool(mute):
                    self._hw_mute.set_active(bool(mute))
                    _set_mute_icon(self._hw_mute)
            for w in self._device_widgets.values():
                if not w.get("is_master"):
                    continue
                sc = w.get("scale")
                if sc is not None and int(round(sc.get_value())) != pct:
                    sc.set_value(pct)
                if mute is not None:
                    mb = w.get("mute")
                    if mb is not None and bool(mb.get_active()) != bool(mute):
                        mb.set_active(bool(mute))
                        _set_mute_icon(mb)
        finally:
            self._building = False

    def _bump_interact(self, ms: int = 700) -> None:
        self._interacting = True
        old = self._interact_timer
        if old is not None:
            try:
                GLib.source_remove(old)
            except Exception:
                pass

        def _clear() -> bool:
            self._interact_timer = None
            # Keep interacting while a drag button is held.
            if self._drag_keys:
                self._interact_timer = GLib.timeout_add(200, _clear)
                return False
            self._interacting = False
            return False

        self._interact_timer = GLib.timeout_add(ms, _clear)

    def _clear(self, box: Gtk.Box) -> None:
        for child in list(box.get_children()):
            box.remove(child)

    def _schedule_vol(self, key: str, args: tuple[str, ...], delay_ms: int = 30) -> None:
        self._touch_local(key)
        old = self._vol_timers.pop(key, None)
        if old is not None:
            try:
                GLib.source_remove(old)
            except Exception:
                pass

        def _fire() -> bool:
            self._vol_timers.pop(key, None)
            ctl_async(*args)
            return False

        self._vol_timers[key] = GLib.timeout_add(delay_ms, _fire)

    def refresh(self) -> None:
        state = fetch_state()
        self._building = True
        try:
            st = dict(state.get("status") or {})
            # Keep optimistic Master HW while the user still owns the fader /
            # waybar gesture — daemon status can lag a probe behind.
            if self._is_local("hw"):
                prev = getattr(self, "_last_status", None) or {}
                if prev.get("hw_volume_pct") is not None:
                    st["hw_volume_pct"] = prev["hw_volume_pct"]
                if "hw_mute" in prev:
                    st["hw_mute"] = prev["hw_mute"]
            self._last_status = st
            self._last_sinks = list(state.get("sinks") or [])
            self._tracks_cache = list(state.get("tracks") or [])
            # Drop favorites that no longer exist
            alive = {t.get("id") for t in self._tracks_cache}
            new_favs = [p for p in self._favorites if p in alive]
            if new_favs != self._favorites:
                self._favorites = new_favs
                save_favorites(self._favorites)
            self._rebuild_playback(state)
            self._rebuild_tracks()
            self._rebuild_devices(self.out_box, self._last_sinks, kind="sink")
            self._rebuild_devices(self.in_box, state.get("sources") or [], kind="source")
        finally:
            self._building = False
        self.show_all()

    def _make_fader_card(
        self,
        *,
        key: str,
        title: str,
        vol_ui: float,
        muted: bool,
        icon_widget: Gtk.Widget | None,
        on_vol,
        on_mute,
        favorited: bool = False,
        on_fav=None,
        badge: str | None = None,
        vol_max: float = VOL_MAX,
    ) -> Gtk.Box:
        card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        card.get_style_context().add_class("stream-card")
        if favorited:
            card.get_style_context().add_class("favorited")
        card.set_size_request(CARD_W, -1)
        card.set_hexpand(False)

        if icon_widget is not None:
            card.pack_start(icon_widget, False, False, 0)

        name = Gtk.Label(label=title, xalign=0.5)
        name.get_style_context().add_class("stream-name")
        name.set_ellipsize(Pango.EllipsizeMode.END)
        name.set_max_width_chars(9)
        card.pack_start(name, False, False, 0)

        if badge:
            meta = Gtk.Label(label=badge, xalign=0.5)
            meta.get_style_context().add_class("stream-meta")
            card.pack_start(meta, False, False, 0)

        fader_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=0)
        fader_row.get_style_context().add_class("fader-row")
        fader_row.set_halign(Gtk.Align.CENTER)

        scale = Gtk.Scale.new_with_range(Gtk.Orientation.VERTICAL, 0, vol_max, 1)
        scale.set_inverted(True)
        scale.set_value(min(float(vol_ui), vol_max))
        scale.set_draw_value(False)
        scale.set_vexpand(True)
        scale.get_style_context().add_class("stream-fader")
        _bind_scale_scroll(scale, vol_max=vol_max)
        self._wire_scale_guard(scale, key)
        scale.connect("value-changed", on_vol)
        fader_row.pack_start(scale, False, False, 0)
        card.pack_start(fader_row, True, True, 0)

        # Live volume % (updates on drag + daemon refresh) — replaces the old side meter.
        pct = Gtk.Label(label=f"{int(min(vol_ui, vol_max))}%", xalign=0.5)
        pct.get_style_context().add_class("stream-pct")
        scale.connect(
            "value-changed",
            lambda sc, lab: lab.set_text(f"{int(sc.get_value())}%"),
            pct,
        )
        card.pack_start(pct, False, False, 0)

        if on_fav is not None:
            star = make_star_toggle(favorited)
            star.connect("toggled", on_fav)
            card.pack_start(star, False, False, 0)

        mute = make_icon_toggle(muted)
        mute.connect("toggled", on_mute)
        card.pack_start(mute, False, False, 0)

        self._stream_widgets[key] = {
            "scale": scale,
            "mute": mute,
            "pct": pct,
        }
        return card

    def _rebuild_playback(self, state: dict) -> None:
        items = state.get("streams") or []
        st = state.get("status") or {}
        hw_pct = int(st.get("hw_volume_pct") or 0)
        hw_mute = bool(st.get("hw_mute"))
        fav_tracks = [t for t in self._tracks_cache if t.get("id") in self._favorites]
        fav_order = {pid: i for i, pid in enumerate(self._favorites)}
        fav_tracks.sort(key=lambda t: fav_order.get(t.get("id"), 999))

        fp = (
            tuple(
                (int(s["index"]), s.get("name"), s.get("icon_name"), s.get("binary"))
                for s in items
            ),
            tuple(
                (t.get("id"), round(float(t.get("gain_db") or 0), 2), bool(t.get("mute")))
                for t in fav_tracks
            ),
            min(hw_pct, int(HW_VOL_MAX)),
            hw_mute,
        )
        if (
            self._play_fp is not None
            and self._play_fp[0] == fp[0]
            and self._play_fp[1] == fp[1]
            and self._stream_widgets
            and any(isinstance(c, Gtk.ScrolledWindow) for c in self.play_box.get_children())
        ):
            self._patch_playback_values(st, items, fav_tracks)
            self._play_fp = fp
            return

        self._play_fp = fp
        # Keep only widgets we rebuild
        self._stream_widgets = {
            k: v for k, v in self._stream_widgets.items() if k.startswith("keep")
        }
        self._stream_widgets.clear()
        self._clear(self.play_box)

        hw = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        hw.get_style_context().add_class("hw-card")
        if not st:
            lab = Gtk.Label(label="Daemon offline", xalign=0)
            lab.get_style_context().add_class("empty")
            hw.pack_start(lab, True, True, 0)
        else:
            title = Gtk.Label(label="Output", xalign=0)
            title.get_style_context().add_class("hw-label")
            hw.pack_start(title, False, False, 0)
            row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
            scale = Gtk.Scale.new_with_range(
                Gtk.Orientation.HORIZONTAL, 0, HW_VOL_MAX, 1
            )
            scale.set_value(min(hw_pct, int(HW_VOL_MAX)))
            scale.set_draw_value(True)
            scale.set_value_pos(Gtk.PositionType.RIGHT)
            scale.get_style_context().add_class("horizontal")
            # Relative up/down scroll — never racing absolute set from the wheel.
            _bind_master_hw_scroll(scale, row, self._apply_hw_notches)
            self._wire_scale_guard(scale, "hw")
            scale.connect("value-changed", self._on_hw_vol)
            mute = make_icon_toggle(hw_mute)
            mute.connect("toggled", self._on_hw_mute)
            row.pack_start(scale, True, True, 0)
            row.pack_end(mute, False, False, 0)
            hw.pack_start(row, False, False, 0)
            self._hw_scale = scale
            self._hw_mute = mute
        self.play_box.pack_start(hw, False, False, 0)

        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.NEVER)
        scroll.set_overlay_scrolling(True)
        n = max(1, len(items) + len(fav_tracks))
        width = min(440, max(240, n * (CARD_W + 10) + 16))
        scroll.set_min_content_width(width)
        scroll.set_max_content_width(560)
        scroll.set_min_content_height(210)
        scroll.get_style_context().add_class("stream-scroll")
        _bind_hscroll_wheel(scroll)
        streams = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=0)
        scroll.add(streams)
        self.play_box.pack_start(scroll, True, True, 0)

        if not items and not fav_tracks:
            lab = Gtk.Label(label="No app playback — star tracks on Tracks")
            lab.get_style_context().add_class("empty")
            streams.pack_start(lab, True, True, 0)
            return

        # Favorite mixer tracks first (leftmost)
        for t in fav_tracks:
            tid = str(t.get("id"))
            key = f"track:{tid}"
            vol_ui = db_to_ui(float(t.get("gain_db") or 0.0))
            ic = icon_image("audio-speakers-symbolic", "🎚")
            ic.get_style_context().add_class("app-icon")
            card = self._make_fader_card(
                key=key,
                title=t.get("name") or "Track",
                vol_ui=vol_ui,
                muted=bool(t.get("mute")),
                icon_widget=ic,
                on_vol=lambda sc, i=tid: self._on_track_vol(sc, i),
                on_mute=lambda btn, i=tid: self._on_track_mute(btn, i),
                favorited=True,
                badge="track",
            )
            streams.pack_start(card, False, False, 0)

        for s in items:
            idx = int(s["index"])
            key = f"si:{idx}"
            card = self._make_fader_card(
                key=key,
                title=s.get("name") or "App",
                vol_ui=float(s.get("volume_pct") or 0),
                muted=bool(s.get("mute")),
                icon_widget=resolve_app_icon(s, 26),
                on_vol=lambda sc, i=idx: self._on_stream_vol(sc, i),
                on_mute=lambda btn, i=idx: self._on_stream_mute(btn, i),
            )
            streams.pack_start(card, False, False, 0)

    def _patch_playback_values(
        self, st: dict, items: list, fav_tracks: list
    ) -> None:
        if st and hasattr(self, "_hw_scale") and not self._is_local("hw"):
            self._building = True
            try:
                self._hw_scale.set_value(
                    min(int(st.get("hw_volume_pct") or 0), int(HW_VOL_MAX))
                )
                if hasattr(self, "_hw_mute"):
                    self._hw_mute.set_active(bool(st.get("hw_mute")))
                    _set_mute_icon(self._hw_mute)
            finally:
                self._building = False
        for t in fav_tracks:
            key = f"track:{t.get('id')}"
            if self._is_local(key):
                continue
            w = self._stream_widgets.get(key)
            if not w:
                continue
            self._building = True
            try:
                vol = db_to_ui(float(t.get("gain_db") or 0))
                w["scale"].set_value(vol)
                w["pct"].set_text(f"{int(vol)}%")
                w["mute"].set_active(bool(t.get("mute")))
                _set_mute_icon(w["mute"])
            finally:
                self._building = False
        for s in items:
            key = f"si:{int(s['index'])}"
            if self._is_local(key):
                continue
            w = self._stream_widgets.get(key)
            if not w:
                continue
            self._building = True
            try:
                vol = int(s.get("volume_pct") or 0)
                w["scale"].set_value(vol)
                w["pct"].set_text(f"{vol}%")
                w["mute"].set_active(bool(s.get("mute")))
                _set_mute_icon(w["mute"])
            finally:
                self._building = False

    def _rebuild_tracks(self) -> None:
        tracks = self._tracks_cache
        fp = tuple(
            (
                t.get("id"),
                t.get("name"),
                round(float(t.get("gain_db") or 0), 2),
                bool(t.get("mute")),
                t.get("id") in self._favorites,
            )
            for t in tracks
        )
        if self._tracks_fp == fp and self.tracks_box.get_children():
            return
        self._tracks_fp = fp
        self._clear(self.tracks_box)

        hint = Gtk.Label(
            label="Star a track to show it on Playback for quick gain.",
            xalign=0,
        )
        hint.get_style_context().add_class("section-hint")
        hint.set_line_wrap(True)
        self.tracks_box.pack_start(hint, False, False, 0)

        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.NEVER)
        scroll.set_overlay_scrolling(True)
        n = max(1, len(tracks))
        width = min(440, max(240, n * (CARD_W + 10) + 16))
        scroll.set_min_content_width(width)
        scroll.set_max_content_width(560)
        scroll.set_min_content_height(260)
        scroll.get_style_context().add_class("stream-scroll")
        _bind_hscroll_wheel(scroll)
        strips = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=0)
        scroll.add(strips)
        self.tracks_box.pack_start(scroll, True, True, 0)

        if not tracks:
            lab = Gtk.Label(label="No mixer tracks")
            lab.get_style_context().add_class("empty")
            strips.pack_start(lab, True, True, 0)
            return

        for t in tracks:
            tid = str(t.get("id"))
            starred = tid in self._favorites
            kind = t.get("kind") or "track"
            vol_ui = db_to_ui(float(t.get("gain_db") or 0.0))
            ic = icon_image(
                "audio-volume-high-symbolic" if kind == "master" else "audio-speakers-symbolic",
                "🎚",
            )
            ic.get_style_context().add_class("app-icon")
            card = self._make_fader_card(
                key=f"tracks-tab:{tid}",
                title=t.get("name") or "Track",
                vol_ui=vol_ui,
                muted=bool(t.get("mute")),
                icon_widget=ic,
                on_vol=lambda sc, i=tid: self._on_track_vol(sc, i),
                on_mute=lambda btn, i=tid: self._on_track_mute(btn, i),
                favorited=starred,
                on_fav=lambda btn, i=tid: self._on_fav_toggle(btn, i),
                badge="Master" if kind == "master" else "Track",
            )
            strips.pack_start(card, False, False, 0)

    def _rebuild_devices(self, box: Gtk.Box, devices: list, kind: str) -> None:
        # Identity / roles only — live volume/mute are patched (avoids scale recreate).
        fp = tuple(
            (
                d.get("name"),
                bool(d.get("is_default")),
                bool(d.get("is_master")),
            )
            for d in devices
        )
        attr = "_out_fp" if kind == "sink" else "_in_fp"
        if getattr(self, attr) == fp and box.get_children() and self._device_widgets:
            self._patch_device_values(devices, kind)
            return
        setattr(self, attr, fp)

        # Drop prior device widget keys for this kind.
        prefix = f"{kind}:"
        self._device_widgets = {
            k: v for k, v in self._device_widgets.items() if not k.startswith(prefix)
        }
        self._clear(box)
        if kind == "sink":
            hint_txt = (
                "Default = where apps open · HW out = BusChain Master destination. "
                "They can be different devices."
            )
        else:
            hint_txt = "Tap Default to set the system default input."
        hint = Gtk.Label(label=hint_txt, xalign=0)
        hint.get_style_context().add_class("section-hint")
        hint.set_line_wrap(True)
        box.pack_start(hint, False, False, 0)

        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        scroll.set_overlay_scrolling(True)
        scroll.set_min_content_width(380)
        scroll.set_min_content_height(260)
        scroll.get_style_context().add_class("stream-scroll")
        _bind_vscroll_yield_to_scale(scroll, vol_max=HW_VOL_MAX)
        listbox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        scroll.add(listbox)
        box.pack_start(scroll, True, True, 0)

        if not devices:
            lab = Gtk.Label(label="No devices")
            lab.get_style_context().add_class("empty")
            listbox.pack_start(lab, True, True, 0)
            return

        for d in devices:
            card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
            card.get_style_context().add_class("device-card")
            if d.get("is_default") or d.get("is_master"):
                card.get_style_context().add_class("device-active")

            top = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
            labels = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
            # Titles only — never dump alsa/node codenames into the UI.
            title = Gtk.Label(label=d.get("desc") or d.get("name") or "Device", xalign=0)
            title.get_style_context().add_class("device-title")
            title.set_ellipsize(Pango.EllipsizeMode.END)
            badges = []
            if d.get("is_default"):
                badges.append("Default")
            if kind == "sink" and d.get("is_master"):
                badges.append("HW out")
            if badges:
                meta = Gtk.Label(label=" · ".join(badges), xalign=0)
                meta.get_style_context().add_class("device-meta")
                labels.pack_start(title, True, True, 0)
                labels.pack_start(meta, True, True, 0)
            else:
                labels.pack_start(title, True, True, 0)
            top.pack_start(labels, True, True, 0)

            name = d["name"]
            actions = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
            if kind == "sink":
                # Independent roles — matching egui Output devices.
                def_btn = Gtk.Button(label="Default")
                def_btn.get_style_context().add_class("use-btn")
                def_btn.get_style_context().add_class("role-default")
                if d.get("is_default"):
                    def_btn.get_style_context().add_class("role-active")
                def_btn.set_relief(Gtk.ReliefStyle.NONE)
                def_btn.set_tooltip_text(
                    "System default sink — apps open onto this device"
                )
                def_btn.connect("clicked", self._on_set_default_sink, name)
                actions.pack_start(def_btn, False, False, 0)

                hw_btn = Gtk.Button(label="HW out")
                hw_btn.get_style_context().add_class("use-btn")
                hw_btn.get_style_context().add_class("role-hw")
                if d.get("is_master"):
                    hw_btn.get_style_context().add_class("role-active")
                hw_btn.set_relief(Gtk.ReliefStyle.NONE)
                hw_btn.set_tooltip_text(
                    "BusChain Master HW out — mixer plays to this device"
                )
                hw_btn.connect("clicked", self._on_set_master_hw, name)
                actions.pack_start(hw_btn, False, False, 0)
            else:
                use = Gtk.Button(label="Default")
                use.get_style_context().add_class("use-btn")
                use.get_style_context().add_class("role-default")
                if d.get("is_default"):
                    use.get_style_context().add_class("role-active")
                use.set_relief(Gtk.ReliefStyle.NONE)
                use.set_tooltip_text("System default source")
                use.connect("clicked", self._on_use_source, name)
                actions.pack_start(use, False, False, 0)
            top.pack_end(actions, False, False, 0)
            card.pack_start(top, False, False, 0)

            row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
            scale = Gtk.Scale.new_with_range(
                Gtk.Orientation.HORIZONTAL, 0, HW_VOL_MAX, 1
            )
            vol_pct = int(d.get("volume_pct") or 0)
            muted = bool(d.get("mute"))
            # Master HW badge must show the same % as Playback Output / status.
            if kind == "sink" and d.get("is_master"):
                st = getattr(self, "_last_status", None) or {}
                if st.get("hw_volume_pct") is not None:
                    vol_pct = int(st.get("hw_volume_pct") or 0)
                if "hw_mute" in st:
                    muted = bool(st.get("hw_mute"))
            scale.set_value(min(vol_pct, int(HW_VOL_MAX)))
            scale.set_draw_value(True)
            scale.set_value_pos(Gtk.PositionType.RIGHT)
            scale.get_style_context().add_class("horizontal")
            is_master_hw = kind == "sink" and bool(d.get("is_master"))
            if is_master_hw:
                _bind_master_hw_scroll(scale, row, self._apply_hw_notches)
            else:
                _bind_scale_scroll(scale, vol_max=HW_VOL_MAX)
                _bind_scroll_on_container(row, scale, vol_max=HW_VOL_MAX)
            # Master HW shares Playback / waybar key "hw" so patch guards match.
            guard = "hw" if is_master_hw else f"{kind}:{name}"
            self._wire_scale_guard(scale, guard)
            if kind == "sink":
                scale.connect("value-changed", self._on_sink_vol, name)
            else:
                scale.connect("value-changed", self._on_source_vol, name)
            mute = make_icon_toggle(muted)
            if kind == "sink":
                mute.connect("toggled", self._on_sink_mute, name)
            else:
                mute.connect("toggled", self._on_source_mute, name)
            row.pack_start(scale, True, True, 0)
            row.pack_end(mute, False, False, 0)
            card.pack_start(row, False, False, 0)
            listbox.pack_start(card, False, False, 0)
            self._device_widgets[f"{kind}:{name}"] = {
                "scale": scale,
                "mute": mute,
                "is_master": bool(d.get("is_master")),
            }

    def _patch_device_values(self, devices: list, kind: str) -> None:
        st = getattr(self, "_last_status", None) or {}
        for d in devices:
            name = d.get("name")
            if not name:
                continue
            dkey = f"{kind}:{name}"
            guard = (
                "hw"
                if kind == "sink" and (d.get("is_master") or False)
                else dkey
            )
            if self._is_local(guard):
                continue
            w = self._device_widgets.get(dkey)
            if w and w.get("is_master") and self._is_local("hw"):
                continue
            if not w:
                continue
            vol_pct = int(d.get("volume_pct") or 0)
            muted = bool(d.get("mute"))
            if kind == "sink" and (d.get("is_master") or w.get("is_master")):
                if st.get("hw_volume_pct") is not None:
                    vol_pct = int(st.get("hw_volume_pct") or 0)
                if "hw_mute" in st:
                    muted = bool(st.get("hw_mute"))
            self._building = True
            try:
                w["scale"].set_value(min(vol_pct, int(HW_VOL_MAX)))
                w["mute"].set_active(muted)
                _set_mute_icon(w["mute"])
            finally:
                self._building = False

    # —— handlers ——

    def _apply_hw_notches(self, notches: int) -> None:
        """Wheel / touchpad → relative daemon Adjust (matches waybar up/down)."""
        if notches == 0 or self._building:
            return
        self._bump_interact()
        # Prefer live thumb; fall back to cached status.
        if hasattr(self, "_hw_scale"):
            cur = int(round(self._hw_scale.get_value()))
        else:
            cur = int(
                (getattr(self, "_last_status", None) or {}).get("hw_volume_pct") or 0
            )
        pct = cur
        if notches > 0:
            for _ in range(notches):
                pct = int(snap_step_volume(pct, 1, vol_max=HW_VOL_MAX))
        else:
            for _ in range(-notches):
                pct = int(snap_step_volume(pct, -1, vol_max=HW_VOL_MAX))
        self._touch_local("hw", 1100)
        self._sync_hw_ui(pct)
        # One Adjust per notch — ordered ctl queue, never absolute set races.
        step = "up" if notches > 0 else "down"
        for _ in range(abs(notches)):
            ctl_queue("hw-vol", step)

    def _on_hw_vol(self, scale: Gtk.Scale) -> None:
        if self._building:
            return
        # Drag only — wheel uses `_apply_hw_notches` (relative up/down).
        self._bump_interact()
        pct = min(int(round(scale.get_value())), int(HW_VOL_MAX))
        self._touch_local("hw", 1100)
        self._sync_hw_ui(pct)
        ctl_queue("hw-vol", "set", str(pct))

    def _on_hw_mute(self, btn: Gtk.ToggleButton) -> None:
        if self._building:
            return
        self._bump_interact()
        muted = bool(btn.get_active())
        self._touch_local("hw", 1100)
        self._sync_hw_ui(
            int((getattr(self, "_last_status", None) or {}).get("hw_volume_pct") or 0),
            mute=muted,
        )
        ctl_queue("hw-vol", "mute", "on" if muted else "off")
        _set_mute_icon(btn)

    def _on_stream_vol(self, scale: Gtk.Scale, index: int) -> None:
        if self._building:
            return
        self._bump_interact()
        self._schedule_vol(
            f"si:{index}",
            ("playback", "vol", str(index), str(int(scale.get_value()))),
        )

    def _on_stream_mute(self, btn: Gtk.ToggleButton, index: int) -> None:
        if self._building:
            return
        self._bump_interact()
        ctl_async(
            "playback", "mute", str(index), "on" if btn.get_active() else "off"
        )
        _set_mute_icon(btn)

    def _on_track_vol(self, scale: Gtk.Scale, track_id: str) -> None:
        if self._building:
            return
        self._bump_interact()
        db = ui_to_db(scale.get_value())
        # Key must match stream widget / patch key (`track:…`).
        self._schedule_vol(
            f"track:{track_id}", ("track", "vol", track_id, f"{db:.2f}")
        )

    def _on_track_mute(self, btn: Gtk.ToggleButton, track_id: str) -> None:
        if self._building:
            return
        self._bump_interact()
        ctl_async("track", "mute", track_id, "on" if btn.get_active() else "off")
        _set_mute_icon(btn)

    def _on_fav_toggle(self, btn: Gtk.ToggleButton, track_id: str) -> None:
        if self._building:
            return
        self._bump_interact(400)
        if btn.get_active():
            if track_id not in self._favorites:
                self._favorites.append(track_id)
        else:
            self._favorites = [p for p in self._favorites if p != track_id]
        _set_star_icon(btn)
        save_favorites(self._favorites)
        # Force playback + tracks rebuild with new favorite order
        self._play_fp = None
        self._tracks_fp = None
        self.refresh()

    def _refresh_devices_soon(self) -> None:
        def _later() -> bool:
            self.refresh()
            return False

        GLib.timeout_add(250, _later)

    def _on_set_default_sink(self, _btn, name: str) -> None:
        """System default only — does not move BusChain Master HW."""
        self._bump_interact(1200)
        for d in getattr(self, "_last_sinks", []) or []:
            d["is_default"] = d.get("name") == name
        self._out_fp = None
        ctl_async("default", "sink", name)
        self._refresh_devices_soon()

    def _on_set_master_hw(self, _btn, name: str) -> None:
        """Master HW out only — does not change the system default sink."""
        self._bump_interact(1200)
        desc = name
        for d in getattr(self, "_last_sinks", []) or []:
            is_hw = d.get("name") == name
            d["is_master"] = is_hw
            if is_hw:
                desc = d.get("desc") or name
        st = getattr(self, "_last_status", None)
        if isinstance(st, dict):
            st["master_hw"] = name
            st["master_hw_desc"] = desc
        self._out_fp = None
        # Force Playback HW card title/value rebuild on next refresh.
        self._play_fp = None
        ctl_async("master-hw", "set", name)
        self._refresh_devices_soon()

    def _on_use_source(self, _btn, name: str) -> None:
        self._bump_interact(1200)
        ctl_async("default", "source", name)
        self._refresh_devices_soon()

    def _master_hw_name(self) -> str | None:
        st = getattr(self, "_last_status", None) or {}
        name = st.get("master_hw") or st.get("master_hw_sink")
        if name:
            return str(name)
        for d in getattr(self, "_last_sinks", []) or []:
            if d.get("is_master") and d.get("name"):
                return str(d["name"])
        return None

    def _on_sink_vol(self, scale: Gtk.Scale, name: str) -> None:
        if self._building:
            return
        self._bump_interact()
        pct = min(int(round(scale.get_value())), int(HW_VOL_MAX))
        # Master HW drag → absolute set; wheel uses `_apply_hw_notches`.
        if self._master_hw_name() == name:
            self._touch_local("hw", 1100)
            self._sync_hw_ui(pct)
            ctl_queue("hw-vol", "set", str(pct))
        else:
            self._schedule_vol(f"sink:{name}", ("sink", "vol", name, str(pct)))

    def _on_source_vol(self, scale: Gtk.Scale, name: str) -> None:
        if self._building:
            return
        self._bump_interact()
        pct = min(int(round(scale.get_value())), int(HW_VOL_MAX))
        # Key must match device widget / patch key (`source:…`).
        self._schedule_vol(f"source:{name}", ("source", "vol", name, str(pct)))

    def _on_sink_mute(self, btn: Gtk.ToggleButton, name: str) -> None:
        if self._building:
            return
        self._bump_interact()
        muted = bool(btn.get_active())
        if self._master_hw_name() == name:
            self._touch_local("hw", 1100)
            self._sync_hw_ui(
                int(
                    (getattr(self, "_last_status", None) or {}).get("hw_volume_pct")
                    or 0
                ),
                mute=muted,
            )
            ctl_async("hw-vol", "mute", "on" if muted else "off")
        else:
            ctl_async("sink", "mute", name, "on" if muted else "off")
        _set_mute_icon(btn)

    def _on_source_mute(self, btn: Gtk.ToggleButton, name: str) -> None:
        if self._building:
            return
        self._bump_interact()
        ctl_async("source", "mute", name, "on" if btn.get_active() else "off")
        _set_mute_icon(btn)


def main() -> int:
    # Before Gtk.init: WhiteSur-dark lacks `image-missing` and aborts on any
    # missing symbolic icon. Force a theme that ships the fallback.
    os.environ.setdefault("GTK_ICON_THEME_NAME", "Adwaita")
    os.environ.setdefault("GTK_THEME", os.environ.get("GTK_THEME", "Adwaita"))

    # --toggle: close if live, else open.
    # --open / bare invoke: open only (never treat "already open" as failure).
    # Tray implements click-toggle by closing via PID or spawning --open — it
    # must not use "toggle exited 0" as proof GTK is unavailable.
    if "--toggle" in sys.argv:
        pid = already_running()
        if pid is not None:
            try:
                os.kill(pid, signal.SIGTERM)
            except OSError:
                PID_FILE.unlink(missing_ok=True)
            return 0
    elif "--open" in sys.argv or len(sys.argv) == 1:
        pid = already_running()
        if pid is not None:
            # Already showing — no-op success (raise would need IPC).
            return 0

    t_main = time.monotonic()
    claim_pid()
    _lat(f"claim_pid {int((time.monotonic() - t_main) * 1000)}ms")
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    signal.signal(signal.SIGINT, lambda *_: sys.exit(0))

    Gtk.init(sys.argv)
    _lat(f"Gtk.init {int((time.monotonic() - t_main) * 1000)}ms")
    load_css()
    win = Mixer()
    win.show_all()
    _lat(f"show_all {int((time.monotonic() - t_main) * 1000)}ms")
    GLib.idle_add(win._initial_refresh)
    Gtk.main()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
