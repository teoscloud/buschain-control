use buschain_control::ipc::{Client, Request, Response};

fn usage() -> ! {
    eprintln!(
        "buschain-ctl — talk to BusChain Control (tray app embedded IPC)\n\n\
         Usage:\n\
           buschain-ctl ping|status\n\
           buschain-ctl mixer\n\
           buschain-ctl hw-vol get|set <pct>|up [n]|down [n]|mute on|off|toggle\n\
           buschain-ctl playback list|vol <index> <pct>|mute <index> on|off|toggle|move <index> <sink>\n\
           buschain-ctl track vol <id> <db>|mute <id> on|off|toggle\n\
           buschain-ctl devices list\n\
           buschain-ctl default sink|source <name>\n\
           buschain-ctl master-hw set <sink-name>\n\
           buschain-ctl sink vol <name> <pct>|mute <name> on|off\n\
           buschain-ctl source vol <name> <pct>|mute <name> on|off\n\
           buschain-ctl session list|load <slug>|save|save-as <name>|delete <slug>\n\
           buschain-ctl popup playback\n\
           buschain-ctl apply|shutdown\n\
           buschain-ctl recover-audio   (restore HW default/unmute; no tray required)\n"
    );
    std::process::exit(2);
}

/// Standalone recovery when Quit/crash left a hollow PipeWire session.
/// Does not require buschain-control to be running.
fn recover_desktop_audio() -> i32 {
    use std::process::Command;

    let sinks = Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let sources = Command::new("pactl")
        .args(["list", "short", "sources"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();

    let hw_sink = sinks
        .lines()
        .filter_map(|l| l.split('\t').nth(1))
        .find(|n| !n.starts_with("buschain_") && !n.starts_with("shadow_"))
        .unwrap_or("")
        .to_string();
    let hw_src = sources
        .lines()
        .filter_map(|l| l.split('\t').nth(1))
        .find(|n| {
            !n.starts_with("buschain_")
                && !n.starts_with("shadow_")
                && !n.ends_with(".monitor")
        })
        .unwrap_or("")
        .to_string();

    if hw_sink.is_empty() {
        eprintln!("recover-audio: no non-buschain sink found");
        return 1;
    }

    let _ = Command::new("pactl")
        .args(["set-default-sink", &hw_sink])
        .status();
    let _ = Command::new("pactl")
        .args(["set-sink-mute", &hw_sink, "0"])
        .status();
    if !hw_src.is_empty() {
        let _ = Command::new("pactl")
            .args(["set-default-source", &hw_src])
            .status();
        let _ = Command::new("pactl")
            .args(["set-source-mute", &hw_src, "0"])
            .status();
    }

    // Move streams off buschain_* onto HW.
    if let Ok(out) = Command::new("pactl")
        .args(["list", "short", "sink-inputs"])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let mut parts = line.split('\t');
            let Some(idx) = parts.next() else { continue };
            let _ = Command::new("pactl")
                .args(["move-sink-input", idx, &hw_sink])
                .status();
        }
    }

    // Best-effort unload Pulse modules that still name buschain_*.
    if let Ok(out) = Command::new("pactl")
        .args(["list", "modules", "short"])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let mut parts = line.splitn(3, '\t');
            let Some(idx) = parts.next() else { continue };
            let Some(name) = parts.next() else { continue };
            let args = parts.next().unwrap_or("");
            let buschain = args.contains("buschain_") || args.contains("shadow_");
            let kind = matches!(
                name,
                "module-null-sink" | "module-loopback" | "module-remap-source" | "module-ladspa-sink"
            );
            if buschain && kind {
                let _ = Command::new("pactl")
                    .args(["unload-module", idx])
                    .status();
            }
        }
    }

    println!("recover-audio: default sink → {hw_sink} (unmuted)");
    if !hw_src.is_empty() {
        println!("recover-audio: default source → {hw_src} (unmuted)");
    }
    println!(
        "If still silent: systemctl --user restart wireplumber\n\
         (NixOS rebuild that restarts wireplumber.service has cleared hollow state.)"
    );
    0
}

fn print_status_waybar(st: &buschain_control::ipc::Status) {
    let hw = st
        .master_hw_desc
        .as_deref()
        .or(st.master_hw.as_deref())
        .unwrap_or("Master HW");
    let icon = if st.hw_mute { "󰖁" } else { "󰕾" };
    let class = if st.hw_mute {
        "muted"
    } else if !st.ok {
        "offline"
    } else {
        "online"
    };
    let sink = st.master_hw.as_deref().unwrap_or("").replace('"', "'");
    let pct = st.hw_volume_pct.min(100);
    println!(
        "{{\"text\":\"{} {}%\",\"tooltip\":\"{}\\n{} · {} Hz q{}\\n{}\",\"percentage\":{},\"class\":\"{}\",\"sink\":\"{}\",\"muted\":{}}}",
        icon,
        pct,
        hw.replace('"', "'"),
        st.session_name.replace('"', "'"),
        st.sample_rate,
        st.quantum,
        st.message.replace('"', "'"),
        pct,
        class,
        sink,
        if st.hw_mute { "true" } else { "false" },
    );
}

fn print_mixer_from_snapshot(
    status: Option<&buschain_control::ipc::Status>,
    snapshot: Option<&buschain_control::audio::graph::PwSnapshot>,
    session: Option<&buschain_control::session::Session>,
) {
    let out = buschain_control::mixer_api::build_mixer_json(status, snapshot, session);
    println!("{out}");
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let cmd = args.remove(0);
    if cmd == "recover-audio" {
        std::process::exit(recover_desktop_audio());
    }
    let req = match cmd.as_str() {
        "ping" => Request::Ping,
        "status" => Request::GetStatus,
        "mixer" => Request::GetMixer,
        "hw-vol" => {
            let sub = args.first().map(|s| s.as_str()).unwrap_or("get");
            match sub {
                "get" => Request::GetStatus,
                "set" => {
                    let pct: u32 = args
                        .get(1)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0)
                        .min(100);
                    Request::SetHwVolume { pct }
                }
                "up" => {
                    let n: i32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
                    Request::AdjustHwVolume { delta: n }
                }
                "down" => {
                    let n: i32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
                    Request::AdjustHwVolume { delta: -n }
                }
                "mute" => {
                    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("toggle");
                    let mute = match mode {
                        "on" | "1" | "true" => true,
                        "off" | "0" | "false" => false,
                        "toggle" => match Client::call(&Request::GetStatus) {
                            Ok(Response::Ok {
                                status: Some(st), ..
                            }) => !st.hw_mute,
                            _ => true,
                        },
                        _ => usage(),
                    };
                    Request::SetHwMute { mute }
                }
                _ => usage(),
            }
        }
        "playback" => {
            let sub = args.first().map(|s| s.as_str()).unwrap_or("list");
            match sub {
                "list" => Request::GetSnapshot,
                "vol" => {
                    let index: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let pct: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    Request::SetSinkInputVolume { index, pct }
                }
                "mute" => {
                    let index: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let mode = args.get(2).map(|s| s.as_str()).unwrap_or("toggle");
                    let mute = match mode {
                        "on" | "1" | "true" => true,
                        "off" | "0" | "false" => false,
                        "toggle" => match Client::call(&Request::GetSnapshot) {
                            Ok(Response::Ok {
                                snapshot: Some(snap),
                                ..
                            }) => snap
                                .sink_inputs
                                .iter()
                                .find(|s| s.index == index)
                                .map(|s| !s.mute)
                                .unwrap_or(true),
                            _ => true,
                        },
                        _ => usage(),
                    };
                    Request::SetSinkInputMute { index, mute }
                }
                "move" => {
                    let index: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let sink = args.get(2).cloned().unwrap_or_default();
                    if sink.is_empty() {
                        usage();
                    }
                    Request::MoveSinkInput { index, sink }
                }
                _ => usage(),
            }
        }
        "devices" => Request::GetSnapshot,
        "track" => {
            let sub = args.first().map(|s| s.as_str()).unwrap_or("");
            let id = args
                .get(1)
                .and_then(|s| uuid::Uuid::parse_str(s).ok())
                .unwrap_or_else(uuid::Uuid::nil);
            match sub {
                "vol" => {
                    let gain_db: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                    Request::SetTrackMixer {
                        track_id: id,
                        gain_db: Some(gain_db),
                        mute: None,
                    }
                }
                "mute" => {
                    let mode = args.get(2).map(|s| s.as_str()).unwrap_or("toggle");
                    let mute = match mode {
                        "on" | "1" | "true" => true,
                        "off" | "0" | "false" => false,
                        "toggle" => match Client::call(&Request::GetMixer) {
                            Ok(Response::Mixer { mixer, .. }) => mixer
                                .get("tracks")
                                .and_then(|t| t.as_array())
                                .and_then(|arr| {
                                    arr.iter().find(|t| {
                                        t.get("id").and_then(|v| v.as_str()) == Some(&id.to_string())
                                    })
                                })
                                .and_then(|t| t.get("mute").and_then(|v| v.as_bool()))
                                .map(|m| !m)
                                .unwrap_or(true),
                            Ok(Response::Ok {
                                session: Some(sess),
                                ..
                            }) => sess
                                .tracks
                                .iter()
                                .find(|t| t.id == id)
                                .map(|t| !t.mute)
                                .unwrap_or(true),
                            _ => true,
                        },
                        _ => usage(),
                    };
                    Request::SetTrackMixer {
                        track_id: id,
                        gain_db: None,
                        mute: Some(mute),
                    }
                }
                _ => usage(),
            }
        }
        "default" => {
            let kind = args.first().map(|s| s.as_str()).unwrap_or("");
            let name = args.get(1).cloned().unwrap_or_default();
            if name.is_empty() {
                usage();
            }
            match kind {
                "sink" => Request::SetDefaultSink { name },
                "source" => Request::SetDefaultSource { name },
                _ => usage(),
            }
        }
        "master-hw" => {
            let sub = args.first().map(|s| s.as_str()).unwrap_or("");
            let name = args.get(1).cloned().unwrap_or_default();
            if sub != "set" || name.is_empty() {
                usage();
            }
            Request::SetMasterHw { name }
        }
        "sink" => {
            let sub = args.first().map(|s| s.as_str()).unwrap_or("");
            let name = args.get(1).cloned().unwrap_or_default();
            match sub {
                "vol" => {
                    let pct: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    Request::SetSinkVolume { name, pct }
                }
                "mute" => {
                    let mode = args.get(2).map(|s| s.as_str()).unwrap_or("on");
                    let mute = matches!(mode, "on" | "1" | "true");
                    Request::SetSinkMute { name, mute }
                }
                _ => usage(),
            }
        }
        "source" => {
            let sub = args.first().map(|s| s.as_str()).unwrap_or("");
            let name = args.get(1).cloned().unwrap_or_default();
            match sub {
                "vol" => {
                    let pct: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    Request::SetSourceVolume { name, pct }
                }
                "mute" => {
                    let mode = args.get(2).map(|s| s.as_str()).unwrap_or("on");
                    let mute = matches!(mode, "on" | "1" | "true");
                    Request::SetSourceMute { name, mute }
                }
                _ => usage(),
            }
        }
        "session" => {
            let sub = args.first().map(|s| s.as_str()).unwrap_or("list");
            match sub {
                "list" => Request::SessionList,
                "load" => Request::SessionLoad {
                    slug: args.get(1).cloned().unwrap_or_default(),
                },
                "save" => Request::SessionSave,
                "save-as" => Request::SessionSaveAs {
                    name: args.get(1).cloned().unwrap_or_default(),
                },
                "delete" => Request::SessionDelete {
                    slug: args.get(1).cloned().unwrap_or_default(),
                },
                _ => usage(),
            }
        }
        "popup" => Request::PopupPlayback,
        "apply" => Request::Apply,
        "shutdown" => Request::Shutdown,
        _ => usage(),
    };

    // Waybar on-scroll: poke and exit — never wait for a reply.
    let hw_scroll = cmd == "hw-vol"
        && matches!(
            args.first().map(|s| s.as_str()),
            Some("up") | Some("down")
        );
    if hw_scroll {
        match Client::poke(&req) {
            Ok(()) => return,
            Err(e) => {
                eprintln!("buschain-ctl: {e:#}");
                std::process::exit(1);
            }
        }
    }

    let resp = if matches!(
        cmd.as_str(),
        "hw-vol" | "status" | "ping" | "popup" | "mixer"
    ) {
        Client::call_fast(&req).or_else(|_| Client::call(&req))
    } else {
        Client::call(&req)
    };
    match resp {
        Ok(Response::Mixer { mixer, .. }) => {
            println!("{mixer}");
        }
        Ok(Response::Ok {
            message,
            status,
            sessions,
            snapshot,
            session,
        }) => {
            if cmd == "hw-vol" {
                let sub = args.first().map(|s| s.as_str()).unwrap_or("get");
                if sub == "get" {
                    if let Some(st) = status {
                        println!("{}", st.hw_volume_pct);
                        return;
                    }
                }
                if matches!(sub, "up" | "down" | "set" | "mute") {
                    if let Some(st) = &status {
                        print_status_waybar(st);
                        return;
                    }
                }
            }
            if cmd == "status" || cmd == "ping" {
                if let Some(st) = &status {
                    print_status_waybar(st);
                    return;
                }
            }
            if (cmd == "playback" && args.first().map(|s| s.as_str()) == Some("list"))
                || cmd == "devices"
            {
                let st = status
                    .as_ref()
                    .cloned()
                    .or_else(|| match Client::call(&Request::GetStatus) {
                        Ok(Response::Ok {
                            status: Some(s), ..
                        }) => Some(s),
                        _ => None,
                    });
                print_mixer_from_snapshot(st.as_ref(), snapshot.as_ref(), session.as_ref());
                return;
            }
            if let Some(list) = sessions {
                for s in list {
                    println!("{}\t{}", s.slug, s.name);
                }
                return;
            }
            if let Some(st) = status {
                println!(
                    "{} · {}% · {} · {}",
                    message,
                    st.hw_volume_pct,
                    st.session_name,
                    st.master_hw.as_deref().unwrap_or("-")
                );
            } else {
                println!("{message}");
            }
        }
        Ok(Response::Err { error }) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
        Err(e) => {
            if cmd == "status" || cmd == "ping" {
                let tip = format!("buschain-control not running ({e})")
                    .replace('\\', "\\\\")
                    .replace('"', "'")
                    .replace('\n', " ");
                println!(
                    "{{\"text\":\"vol —\",\"tooltip\":\"{tip}\",\"class\":\"offline\",\"percentage\":0,\"muted\":false,\"sink\":\"\"}}"
                );
                std::process::exit(0);
            }
            eprintln!("buschain-ctl: {e:#}");
            eprintln!("Is buschain-control running? (Hyprland: exec-once = buschain-control --hidden)");
            std::process::exit(1);
        }
    }
}
