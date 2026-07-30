// Master HW scroll strip skeleton — transparent hit target over Waybar pill.
// Geometry env (same as GTK strip):
//   BUSCHAIN_CONTROL_SCROLL_ANCHOR=left|right
//   BUSCHAIN_CONTROL_SCROLL_MARGIN_TOP / _MARGIN_X / _WIDTH / _HEIGHT
// Contract: docs/HANDOVER-QUICKSHELL.md

pragma ComponentBehavior: Bound
import QtQuick
import Quickshell
import Quickshell.Io

PanelWindow {
    id: strip
    // Quant: layer shell top, exclusiveZone -1, position over the vol pill.
    color: "transparent"
    implicitWidth: Number(Quickshell.env("BUSCHAIN_CONTROL_SCROLL_WIDTH") || 110)
    implicitHeight: Number(Quickshell.env("BUSCHAIN_CONTROL_SCROLL_HEIGHT") || 38)

    property string ctl: "buschain-ctl"
    property real accum: 0

    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.LeftButton
        onClicked: popup.running = true
        onWheel: (wheel) => {
            // Accumulate SMOOTH magnitude → ±5% notches (max 2 per event).
            strip.accum += wheel.angleDelta.y;
            let notches = 0;
            while (strip.accum >= 120) {
                notches += 1;
                strip.accum -= 120;
            }
            while (strip.accum <= -120) {
                notches -= 1;
                strip.accum += 120;
            }
            if (notches > 2)
                notches = 2;
            if (notches < -2)
                notches = -2;
            if (notches > 0) {
                for (let i = 0; i < notches; i++)
                    hwUp.running = true;
            } else if (notches < 0) {
                for (let i = 0; i < -notches; i++)
                    hwDown.running = true;
            }
        }
    }

    Process {
        id: hwUp
        command: [strip.ctl, "hw-vol", "up"]
    }
    Process {
        id: hwDown
        command: [strip.ctl, "hw-vol", "down"]
    }
    Process {
        id: popup
        command: [strip.ctl, "popup", "playback"]
    }
}
