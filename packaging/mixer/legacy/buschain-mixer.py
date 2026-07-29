#!/usr/bin/env python3
"""BusChain Control overlay — modern Playback / Tracks / Output / Input."""

from __future__ import annotations

import atexit
import json
import math
import os
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


def ctl_async(*args: str) -> None:
    def _run() -> None:
        try:
            subprocess.run([CTL, *args], capture_output=True, text=True, check=False)
        except FileNotFoundError:
            pass

    threading.Thread(target=_run, daemon=True).start()


def ctl(*args: str) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run([CTL, *args], capture_output=True, text=True, check=False)
    except FileNotFoundError:
        return subprocess.CompletedProcess([CTL, *args], returncode=127, stdout="", stderr="")


def fetch_state() -> dict:
    r = ctl("devices", "list")
    if r.returncode != 0 or not r.stdout.strip():
        r = ctl("playback", "list")
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


def already_running() -> int | None:
    if not PID_FILE.exists():
        return None
    try:
        pid = int(PID_FILE.read_text().strip())
    except ValueError:
        return None
    try:
        os.kill(pid, 0)
        return pid
    except OSError:
        PID_FILE.unlink(missing_ok=True)
        return None


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


def resolve_app_icon(stream: dict, size: int = 28) -> Gtk.Image | None:
    theme = Gtk.IconTheme.get_default()
    for name in _icon_candidates(stream):
        if theme.has_icon(name):
            try:
                pix = theme.load_icon(name, size, 0)
                img = Gtk.Image.new_from_pixbuf(pix)
                img.get_style_context().add_class("app-icon")
                return img
            except GLib.Error:
                continue
    if theme.has_icon("audio-volume-high-symbolic"):
        img = Gtk.Image.new_from_icon_name(
            "audio-volume-high-symbolic", Gtk.IconSize.BUTTON
        )
        img.get_style_context().add_class("app-icon")
        return img
    return None


def icon_image(name: str, fallback: str = "●") -> Gtk.Widget:
    theme = Gtk.IconTheme.get_default()
    if theme.has_icon(name):
        return Gtk.Image.new_from_icon_name(name, Gtk.IconSize.BUTTON)
    lab = Gtk.Label(label=fallback)
    return lab


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
    """+1 volume up, -1 volume down, 0 = ignore. Wheel-up raises volume."""
    if event.direction == Gdk.ScrollDirection.SMOOTH:
        dx = float(getattr(event, "delta_x", 0.0) or 0.0)
        dy = float(getattr(event, "delta_y", 0.0) or 0.0)
        if abs(dy) >= abs(dx):
            if abs(dy) < 0.2:
                return 0
            # negative dy = wheel up → louder
            return 1 if dy < 0 else -1
        if abs(dx) < 0.2:
            return 0
        return 1 if dx < 0 else -1
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


def _bind_scale_scroll(scale: Gtk.Scale, vol_max: float = VOL_MAX) -> None:
    def on_scroll(sc: Gtk.Scale, event) -> bool:
        direction = _scroll_direction(event)
        if direction == 0:
            return True
        sc.set_value(snap_step_volume(sc.get_value(), direction, vol_max=vol_max))
        return True

    def on_release(sc: Gtk.Scale, _event) -> bool:
        v = sc.get_value()
        if abs(v - VOL_SNAP) <= 4.0 and v != VOL_SNAP:
            sc.set_value(min(VOL_SNAP, vol_max))
        return False

    scale.connect("scroll-event", on_scroll)
    scale.connect("button-release-event", on_release)


def _bind_hscroll_wheel(scroll: Gtk.ScrolledWindow) -> None:
    """Playback / Tracks strip rows: mouse wheel pans horizontally by default."""
    scroll.add_events(Gdk.EventMask.SCROLL_MASK | Gdk.EventMask.SMOOTH_SCROLL_MASK)

    def on_scroll(_w, event) -> bool:
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
            elif abs(dy_raw) >= 0.1:
                # Vertical wheel → horizontal pan (touchpads send dy).
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
        self._opened_at = time.monotonic()
        self._play_fp: tuple | None = None
        self._tracks_fp: tuple | None = None
        self._out_fp: tuple | None = None
        self._in_fp: tuple | None = None
        self._stream_widgets: dict[str, dict] = {}
        self._vol_timers: dict[str, int] = {}
        self._favorites = load_favorites()
        self._tracks_cache: list[dict] = []

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

        header = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        titles = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        t = Gtk.Label(label="BUSCHAIN CONTROL", xalign=0)
        t.get_style_context().add_class("title")
        self.subtitle = Gtk.Label(label="click outside · Esc closes", xalign=0)
        self.subtitle.get_style_context().add_class("subtitle")
        titles.pack_start(t, True, True, 0)
        titles.pack_start(self.subtitle, True, True, 0)
        header.pack_start(titles, True, True, 0)
        self.panel.pack_start(header, False, False, 0)

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

        self.refresh()
        GLib.timeout_add(1600, self._tick)

    def _on_key(self, _w, event) -> bool:
        if event.keyval == Gdk.KEY_Escape:
            self.close()
            return True
        return False

    def _on_focus_out(self, _w, _event) -> bool:
        # Ignore the opening click / focus churn from waybar launching us.
        if time.monotonic() - self._opened_at < 0.4:
            return False

        def _close() -> bool:
            if not self.get_window():
                return False
            # Still focused (e.g. transient focus bounce) — keep open.
            if self.has_toplevel_focus():
                return False
            self.close()
            return False

        GLib.timeout_add(80, _close)
        return False

    def _tick(self) -> bool:
        if not self._building and not self._interacting:
            self.refresh()
        return True

    def _bump_interact(self, ms: int = 700) -> None:
        self._interacting = True

        def _clear() -> bool:
            self._interacting = False
            return False

        GLib.timeout_add(ms, _clear)

    def _clear(self, box: Gtk.Box) -> None:
        for child in list(box.get_children()):
            box.remove(child)

    def _schedule_vol(self, key: str, args: tuple[str, ...], delay_ms: int = 50) -> None:
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
            st = state.get("status") or {}
            sess = st.get("session_name") or "—"
            rate = st.get("sample_rate") or "?"
            self.subtitle.set_text(f"{sess} · {rate} Hz · Esc closes")
            self._tracks_cache = list(state.get("tracks") or [])
            # Drop favorites that no longer exist
            alive = {t.get("id") for t in self._tracks_cache}
            new_favs = [p for p in self._favorites if p in alive]
            if new_favs != self._favorites:
                self._favorites = new_favs
                save_favorites(self._favorites)
            self._rebuild_playback(state)
            self._rebuild_tracks()
            self._rebuild_devices(self.out_box, state.get("sinks") or [], kind="sink")
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
            _bind_scale_scroll(scale, vol_max=HW_VOL_MAX)
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
        if st and hasattr(self, "_hw_scale") and not self._interacting:
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
        if self._interacting:
            return
        for t in fav_tracks:
            key = f"track:{t.get('id')}"
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
        fp = tuple(
            (
                d.get("name"),
                int(d.get("volume_pct") or 0),
                bool(d.get("mute")),
                bool(d.get("is_default")),
                bool(d.get("is_master")),
            )
            for d in devices
        )
        attr = "_out_fp" if kind == "sink" else "_in_fp"
        if getattr(self, attr) == fp and box.get_children():
            return
        setattr(self, attr, fp)

        self._clear(box)
        hint = Gtk.Label(
            label="Tap Use to set system default."
            + (" Master HW is BusChain's output." if kind == "sink" else ""),
            xalign=0,
        )
        hint.get_style_context().add_class("section-hint")
        hint.set_line_wrap(True)
        box.pack_start(hint, False, False, 0)

        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        scroll.set_overlay_scrolling(True)
        scroll.set_min_content_width(380)
        scroll.set_min_content_height(260)
        scroll.get_style_context().add_class("stream-scroll")
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
            if d.get("is_master"):
                badges.append("Master HW")
            if badges:
                meta = Gtk.Label(label=" · ".join(badges), xalign=0)
                meta.get_style_context().add_class("device-meta")
                labels.pack_start(title, True, True, 0)
                labels.pack_start(meta, True, True, 0)
            else:
                labels.pack_start(title, True, True, 0)
            top.pack_start(labels, True, True, 0)

            use = Gtk.Button(label="Use")
            use.get_style_context().add_class("use-btn")
            use.set_relief(Gtk.ReliefStyle.NONE)
            name = d["name"]
            if kind == "sink":
                use.connect("clicked", self._on_use_sink, name)
            else:
                use.connect("clicked", self._on_use_source, name)
            top.pack_end(use, False, False, 0)
            card.pack_start(top, False, False, 0)

            row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
            scale = Gtk.Scale.new_with_range(
                Gtk.Orientation.HORIZONTAL, 0, HW_VOL_MAX, 1
            )
            scale.set_value(min(int(d.get("volume_pct") or 0), int(HW_VOL_MAX)))
            scale.set_draw_value(True)
            scale.set_value_pos(Gtk.PositionType.RIGHT)
            scale.get_style_context().add_class("horizontal")
            _bind_scale_scroll(scale, vol_max=HW_VOL_MAX)
            if kind == "sink":
                scale.connect("value-changed", self._on_sink_vol, name)
            else:
                scale.connect("value-changed", self._on_source_vol, name)
            mute = make_icon_toggle(bool(d.get("mute")))
            if kind == "sink":
                mute.connect("toggled", self._on_sink_mute, name)
            else:
                mute.connect("toggled", self._on_source_mute, name)
            row.pack_start(scale, True, True, 0)
            row.pack_end(mute, False, False, 0)
            card.pack_start(row, False, False, 0)
            listbox.pack_start(card, False, False, 0)

    # —— handlers ——

    def _on_hw_vol(self, scale: Gtk.Scale) -> None:
        if self._building:
            return
        self._bump_interact()
        pct = min(int(scale.get_value()), int(HW_VOL_MAX))
        if int(scale.get_value()) != pct:
            scale.set_value(pct)
        self._schedule_vol("hw", ("hw-vol", "set", str(pct)))

    def _on_hw_mute(self, btn: Gtk.ToggleButton) -> None:
        if self._building:
            return
        self._bump_interact()
        ctl_async("hw-vol", "mute", "on" if btn.get_active() else "off")
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
        self._schedule_vol(f"tr:{track_id}", ("track", "vol", track_id, f"{db:.2f}"))

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

    def _on_use_sink(self, _btn, name: str) -> None:
        self._bump_interact(1200)
        ctl_async("default", "sink", name)
        ctl_async("master-hw", "set", name)

        def _later() -> bool:
            self.refresh()
            return False

        GLib.timeout_add(250, _later)

    def _on_use_source(self, _btn, name: str) -> None:
        self._bump_interact(1200)
        ctl_async("default", "source", name)

        def _later() -> bool:
            self.refresh()
            return False

        GLib.timeout_add(250, _later)

    def _on_sink_vol(self, scale: Gtk.Scale, name: str) -> None:
        if self._building:
            return
        self._bump_interact()
        pct = min(int(scale.get_value()), int(HW_VOL_MAX))
        self._schedule_vol(f"sink:{name}", ("sink", "vol", name, str(pct)))

    def _on_source_vol(self, scale: Gtk.Scale, name: str) -> None:
        if self._building:
            return
        self._bump_interact()
        pct = min(int(scale.get_value()), int(HW_VOL_MAX))
        self._schedule_vol(f"src:{name}", ("source", "vol", name, str(pct)))

    def _on_sink_mute(self, btn: Gtk.ToggleButton, name: str) -> None:
        if self._building:
            return
        self._bump_interact()
        ctl_async("sink", "mute", name, "on" if btn.get_active() else "off")
        _set_mute_icon(btn)

    def _on_source_mute(self, btn: Gtk.ToggleButton, name: str) -> None:
        if self._building:
            return
        self._bump_interact()
        ctl_async("source", "mute", name, "on" if btn.get_active() else "off")
        _set_mute_icon(btn)


def main() -> int:
    if "--toggle" in sys.argv or len(sys.argv) == 1:
        pid = already_running()
        if pid is not None:
            os.kill(pid, signal.SIGTERM)
            return 0

    claim_pid()
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    signal.signal(signal.SIGINT, lambda *_: sys.exit(0))

    Gtk.init(sys.argv)
    load_css()
    win = Mixer()
    win.show_all()
    Gtk.main()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
