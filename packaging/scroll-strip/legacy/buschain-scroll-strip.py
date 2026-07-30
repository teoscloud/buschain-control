#!/usr/bin/env python3
"""BusChain Master HW scroll strip — owns wheel notches over the Waybar pill.

Waybar custom on-scroll forkExec discards SMOOTH magnitude (one shell call per
coalesced event). This transparent layer-shell hit target receives the real
delta, applies N×±5% via daemon AdjustHwVolume, and refreshes the pill via
the tray's RTMIN+9 path.

Geometry (env, defaults suit a left-side waybar pill):
  BUSCHAIN_CONTROL_SCROLL_ANCHOR   left|right   (default left)
  BUSCHAIN_CONTROL_SCROLL_MARGIN_TOP  px        (default 0)
  BUSCHAIN_CONTROL_SCROLL_MARGIN_X    px        (default 8)
  BUSCHAIN_CONTROL_SCROLL_WIDTH       px        (default 110)
  BUSCHAIN_CONTROL_SCROLL_HEIGHT      px        (default 38)

Layer-shell exclusive_zone is -1 so Hyprland does not park the strip below
Waybar's reserved band (exclusive_zone 0 → y=bar_height → no hover scroll).
"""

from __future__ import annotations

import atexit
import json
import os
import signal
import socket
import sys
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

from gi.repository import Gdk, Gtk  # noqa: E402

RUNTIME = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "buschain-control"
PID_FILE = RUNTIME / "scroll-strip.pid"
SOCK = RUNTIME / "daemon.sock"

# One discrete mouse notch ≈ |delta_y| 1.0 on libinput/Wayland.
ACCUM_STEP = 1.0
# Cap notches applied from a single Gdk event (touchpad flood).
MAX_NOTCHES_PER_EVENT = 2


def _env_int(name: str, default: int) -> int:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        return int(raw)
    except ValueError:
        return default


def _already_running() -> int | None:
    if not PID_FILE.exists():
        return None
    try:
        pid = int(PID_FILE.read_text().strip())
    except ValueError:
        return None
    if pid <= 0:
        return None
    try:
        os.kill(pid, 0)
    except OSError:
        return None
    # Reject zombies / wrong process.
    try:
        cmd = Path(f"/proc/{pid}/cmdline").read_bytes().replace(b"\x00", b" ").decode()
    except OSError:
        return None
    if "buschain-scroll-strip" not in cmd and "scroll_strip" not in cmd:
        return None
    return pid


def _write_pid() -> None:
    RUNTIME.mkdir(parents=True, exist_ok=True)
    PID_FILE.write_text(f"{os.getpid()}\n")


def _clear_pid() -> None:
    try:
        if PID_FILE.exists() and PID_FILE.read_text().strip() == str(os.getpid()):
            PID_FILE.unlink(missing_ok=True)
    except OSError:
        pass


def poke_adjust(delta: int) -> None:
    """Fire-and-forget AdjustHwVolume (no reply wait)."""
    if delta == 0:
        return
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(0.05)
        s.connect(str(SOCK))
        payload = json.dumps({"op": "adjust_hw_volume", "delta": int(delta)}) + "\n"
        s.sendall(payload.encode())
        s.close()
    except OSError:
        pass


def poke_popup() -> None:
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(0.15)
        s.connect(str(SOCK))
        s.sendall(b'{"op":"popup_playback"}\n')
        s.close()
    except OSError:
        pass


class ScrollStrip(Gtk.Window):
    def __init__(self) -> None:
        super().__init__(title="BusChain Scroll")
        self.set_decorated(False)
        self.set_resizable(False)
        self.set_app_paintable(True)
        self.set_accept_focus(False)
        screen = self.get_screen()
        visual = screen.get_rgba_visual() if screen is not None else None
        if visual is not None:
            self.set_visual(visual)

        self._accum = 0.0
        w = max(40, _env_int("BUSCHAIN_CONTROL_SCROLL_WIDTH", 110))
        h = max(20, _env_int("BUSCHAIN_CONTROL_SCROLL_HEIGHT", 38))
        self.set_size_request(w, h)

        # Transparent hit target — no chrome.
        self.box = Gtk.EventBox()
        self.box.set_visible_window(True)
        self.box.set_above_child(True)
        self.box.set_size_request(w, h)
        self.box.get_style_context().add_class("scroll-strip")
        self.add(self.box)

        self.box.add_events(
            Gdk.EventMask.SCROLL_MASK
            | Gdk.EventMask.SMOOTH_SCROLL_MASK
            | Gdk.EventMask.BUTTON_PRESS_MASK
            | Gdk.EventMask.ENTER_NOTIFY_MASK
        )
        self.box.connect("scroll-event", self._on_scroll)
        self.box.connect("button-press-event", self._on_click)

        if HAS_LAYER:
            GtkLayerShell.init_for_window(self)
            # OVERLAY above Waybar so the strip receives the gesture on the pill.
            GtkLayerShell.set_layer(self, GtkLayerShell.Layer.OVERLAY)
            # Above the bar so we receive the gesture; waybar still paints under us.
            GtkLayerShell.set_namespace(self, "buschain-scroll-strip")
            GtkLayerShell.set_keyboard_mode(self, GtkLayerShell.KeyboardMode.NONE)
            # -1 = cover exclusive zones (waybar). 0 = compositor pushes us below
            # the bar (y=bar_height) so the pill never receives scroll.
            GtkLayerShell.set_exclusive_zone(self, -1)
            anchor = os.environ.get("BUSCHAIN_CONTROL_SCROLL_ANCHOR", "left").lower()
            GtkLayerShell.set_anchor(self, GtkLayerShell.Edge.TOP, True)
            margin_top = _env_int("BUSCHAIN_CONTROL_SCROLL_MARGIN_TOP", 0)
            margin_x = _env_int("BUSCHAIN_CONTROL_SCROLL_MARGIN_X", 8)
            GtkLayerShell.set_margin(self, GtkLayerShell.Edge.TOP, margin_top)
            if anchor in ("right", "end"):
                GtkLayerShell.set_anchor(self, GtkLayerShell.Edge.RIGHT, True)
                GtkLayerShell.set_margin(self, GtkLayerShell.Edge.RIGHT, margin_x)
            else:
                GtkLayerShell.set_anchor(self, GtkLayerShell.Edge.LEFT, True)
                GtkLayerShell.set_margin(self, GtkLayerShell.Edge.LEFT, margin_x)
        else:
            self.set_keep_above(True)
            self.move(8, 0)

        self.connect("destroy", Gtk.main_quit)
        self._load_css()

    def _load_css(self) -> None:
        css = b"""
        window, .scroll-strip {
          background-color: rgba(0, 0, 0, 0.01);
        }
        """
        provider = Gtk.CssProvider()
        provider.load_from_data(css)
        Gtk.StyleContext.add_provider_for_screen(
            Gdk.Screen.get_default(),
            provider,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION,
        )

    def _on_click(self, _w, event) -> bool:
        if event.button == 1:
            poke_popup()
            return True
        return False

    def _on_scroll(self, _w, event) -> bool:
        notches = 0
        if event.direction == Gdk.ScrollDirection.SMOOTH:
            dx = float(getattr(event, "delta_x", 0.0) or 0.0)
            dy = float(getattr(event, "delta_y", 0.0) or 0.0)
            # Wheel up → negative dy → louder. Prefer vertical.
            delta = (-dy) if abs(dy) >= abs(dx) else (-dx)
            if abs(delta) < 1e-9:
                return True
            self._accum += delta
            # Truncate toward zero for whole notches; keep residue.
            if self._accum >= ACCUM_STEP:
                notches = int(self._accum / ACCUM_STEP)
                self._accum -= notches * ACCUM_STEP
            elif self._accum <= -ACCUM_STEP:
                notches = -int((-self._accum) / ACCUM_STEP)
                self._accum -= notches * ACCUM_STEP
        elif event.direction in (Gdk.ScrollDirection.UP, Gdk.ScrollDirection.LEFT):
            notches = 1
        elif event.direction in (Gdk.ScrollDirection.DOWN, Gdk.ScrollDirection.RIGHT):
            notches = -1

        if notches == 0:
            return True

        # Apply in ≤MAX chunks so one touchpad frame cannot jump 50%.
        while notches != 0:
            step = max(-MAX_NOTCHES_PER_EVENT, min(MAX_NOTCHES_PER_EVENT, notches))
            poke_adjust(step * 5)
            notches -= step
        return True


def main() -> int:
    if os.environ.get("BUSCHAIN_CONTROL_SCROLL_STRIP", "1").strip() in (
        "0",
        "false",
        "off",
        "no",
    ):
        return 0

    live = _already_running()
    if live is not None:
        # Already owned — stay single-instance.
        return 0

    if not HAS_LAYER:
        print("buschain-scroll-strip: GtkLayerShell required", file=sys.stderr)
        return 1

    _write_pid()
    atexit.register(_clear_pid)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    signal.signal(signal.SIGINT, lambda *_: sys.exit(0))

    # Wait briefly for tray IPC (spawn races start_embedded).
    for _ in range(50):
        if SOCK.exists():
            break
        time.sleep(0.05)

    win = ScrollStrip()
    win.show_all()
    Gtk.main()
    return 0


if __name__ == "__main__":
    sys.exit(main())
