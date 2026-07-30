// BusChain mixer panel skeleton — unstyled placeholders for Quant.
// Wire into your Quickshell root; replace Process wrappers with your Process/Quickshell APIs.
// Contract: docs/HANDOVER-QUICKSHELL.md
//
pragma ComponentBehavior: Bound
import QtQuick
import Quickshell
import Quickshell.Io

PanelWindow {
    id: mixer
    visible: false
    // Quant: anchors, margins, colors, fonts — all rice-side.

    property var mixerData: ({})
    property string ctl: "buschain-ctl"

    function toggle() {
        visible = !visible;
        if (visible)
            refresh.running = true;
    }

    // Expose to: qs ipc call mixer toggle
    IpcHandler {
        target: "mixer"
        function toggle() {
            mixer.toggle();
        }
    }

    // Poll while open (~200ms). Prefer FileView on mixer.tick when available.
    Timer {
        id: refresh
        interval: 200
        repeat: true
        running: mixer.visible
        onTriggered: poll.running = true
    }

    Process {
        id: poll
        command: [mixer.ctl, "mixer"]
        stdout: StdioCollector {
            onStreamFinished: {
                try {
                    mixer.mixerData = JSON.parse(this.text);
                } catch (e) {}
            }
        }
    }

    // Example mutations (Quant binds these to knobs / faders):
    //   buschain-ctl hw-vol set <pct>
    //   buschain-ctl hw-vol mute toggle
    //   buschain-ctl playback vol <index> <pct>
    //   buschain-ctl track vol <uuid> <db>
    //   buschain-ctl track mute <uuid> toggle
    //   buschain-ctl playback move <index> <sink-or-bus>
    //   buschain-ctl default sink|source <name>
    //   buschain-ctl master-hw set <name>

    Column {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 8
        Text {
            text: "BusChain Mixer (skeleton)"
            color: "white"
        }
        Text {
            text: "HW " + ((mixer.mixerData.status && mixer.mixerData.status.hw_volume_pct) || "?") + "%"
            color: "#ccc"
        }
        Text {
            text: "streams=" + ((mixer.mixerData.streams && mixer.mixerData.streams.length) || 0)
                  + " tracks=" + ((mixer.mixerData.tracks && mixer.mixerData.tracks.length) || 0)
            color: "#888"
        }
    }

    // Optional: wake on daemon tick instead of blind poll
    FileView {
        path: `${Quickshell.env("XDG_RUNTIME_DIR") || "/tmp"}/buschain-control/mixer.tick`
        watchChanges: true
        onFileChanged: if (mixer.visible)
            poll.running = true
    }
}
