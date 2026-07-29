use buschain_control::ipc::{Client, Request, Response};

fn usage() -> ! {
    eprintln!(
        "buschain-ctl — talk to BusChain Control (tray app embedded IPC)\n\n\
         Usage:\n\
           buschain-ctl ping|status\n\
           buschain-ctl hw-vol get|set <pct>|up [n]|down [n]|mute on|off|toggle\n\
           buschain-ctl playback list|vol <index> <pct>|mute <index> on|off|toggle\n\
           buschain-ctl track vol <id> <db>|mute <id> on|off|toggle\n\
           buschain-ctl devices list\n\
           buschain-ctl default sink|source <name>\n\
           buschain-ctl master-hw set <sink-name>\n\
           buschain-ctl sink vol <name> <pct>|mute <name> on|off\n\
           buschain-ctl source vol <name> <pct>|mute <name> on|off\n\
           buschain-ctl session list|load <slug>|save|save-as <name>|delete <slug>\n\
           buschain-ctl popup playback\n\
           buschain-ctl apply|shutdown\n"
    );
    std::process::exit(2);
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
    // Compact waybar label: icon + percent only (device name stays in tooltip).
    // Include sink name so the waybar scroll helper can pactl the Master HW
    // directly without a second IPC round-trip.
    let sink = st.master_hw.as_deref().unwrap_or("").replace('"', "'");
    // Master HW never advertises boost — clamp label + percentage at 100.
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

fn is_buschain_node(name: &str) -> bool {
    name.starts_with("buschain_")
        || name.starts_with("easyeffects_")
        || name.contains("filter-chain")
        || name == "auto_null"
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let cmd = args.remove(0);
    let req = match cmd.as_str() {
        "ping" => Request::Ping,
        "status" => Request::GetStatus,
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
                        // Cached snapshot — never force a full pactl refresh for mute toggle.
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
                        "toggle" => true, // daemon has no cheap read; mixer sends explicit on/off
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

    // HW volume / mute need the short timeout — waybar scroll must not wait on
    // a 5s IPC budget when the daemon is briefly busy.
    let resp = if matches!(cmd.as_str(), "hw-vol" | "status" | "ping") {
        Client::call_fast(&req).or_else(|_| Client::call(&req))
    } else {
        Client::call(&req)
    };
    match resp {
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
                // Scroll path: print JSON for instant module text; waybar script signals once.
                // Do not pkill here — double-signal races make scroll feel sticky.
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
                let st = status.clone().or_else(|| match Client::call(&Request::GetStatus) {
                    Ok(Response::Ok {
                        status: Some(s), ..
                    }) => Some(s),
                    _ => None,
                });
                let sess = session.as_ref();
                let master = sess
                    .and_then(|s| s.master_output.clone())
                    .or_else(|| st.as_ref().and_then(|s| s.master_hw.clone()));
                let default_sink = snapshot
                    .as_ref()
                    .and_then(|s| s.default_sink.clone());
                let default_source = snapshot
                    .as_ref()
                    .and_then(|s| s.default_source.clone());

                let streams: Vec<serde_json::Value> = snapshot
                    .as_ref()
                    .map(|snap| {
                        snap.sink_inputs
                            .iter()
                            .filter(|s| s.is_user_app())
                            .map(|s| {
                                serde_json::json!({
                                    "index": s.index,
                                    "name": s.display_name(),
                                    "meta": s.sink_or_source,
                                    "volume_pct": s.volume_pct,
                                    "mute": s.mute,
                                    "icon_name": s.icon_name,
                                    "binary": s.binary,
                                    "app_id": s.app_id,
                                    "application": s.application,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let sinks: Vec<serde_json::Value> = snapshot
                    .as_ref()
                    .map(|snap| {
                        snap.sinks
                            .iter()
                            .filter(|s| !is_buschain_node(&s.name))
                            .map(|s| {
                                serde_json::json!({
                                    "name": s.name,
                                    "desc": s.description,
                                    "volume_pct": s.volume_pct,
                                    "mute": s.mute,
                                    "is_master": master.as_deref() == Some(s.name.as_str()),
                                    "is_default": default_sink.as_deref() == Some(s.name.as_str()),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let sources: Vec<serde_json::Value> = snapshot
                    .as_ref()
                    .map(|snap| {
                        snap.sources
                            .iter()
                            .filter(|s| !is_buschain_node(&s.name) && !s.name.contains(".monitor"))
                            .map(|s| {
                                serde_json::json!({
                                    "name": s.name,
                                    "desc": s.description,
                                    "volume_pct": s.volume_pct,
                                    "mute": s.mute,
                                    "is_default": default_source.as_deref() == Some(s.name.as_str()),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let tracks: Vec<serde_json::Value> = sess
                    .map(|s| {
                        s.tracks
                            .iter()
                            .map(|t| {
                                let kind = if t.kind.is_master() {
                                    "master"
                                } else {
                                    "track"
                                };
                                serde_json::json!({
                                    "id": t.id.to_string(),
                                    "name": t.name,
                                    "kind": kind,
                                    "gain_db": t.gain_db,
                                    "mute": t.mute,
                                    "bus": t.expected_sink_name(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let out = serde_json::json!({
                    "status": st.as_ref().map(|s| serde_json::json!({
                        "master_hw": s.master_hw,
                        "master_hw_desc": s.master_hw_desc,
                        "hw_volume_pct": s.hw_volume_pct,
                        "hw_mute": s.hw_mute,
                        "session_name": s.session_name,
                        "sample_rate": s.sample_rate,
                    })),
                    "streams": streams,
                    "sinks": sinks,
                    "sources": sources,
                    "tracks": tracks,
                    "default_sink": default_sink,
                    "default_source": default_source,
                });
                println!("{out}");
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
            eprintln!("buschain-ctl: {e:#}");
            eprintln!("Is buschain-control running? (Hyprland: exec-once = buschain-control --hidden)");
            std::process::exit(1);
        }
    }
}
