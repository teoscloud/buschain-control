use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::audio::graph::PwSnapshot;
use crate::audio::live::LiveChange;
use crate::audio::meters::{MeterHub, MeterTap};
use crate::audio::plugin::PluginHost;
use crate::audio::worker::{AudioWorker, Command, Event, SessionAppliedKind};
use crate::design::SpectrumTheme;
use crate::session::Session;
use egui::TextureHandle;
use uuid::Uuid;

/// Post-FX meter sink for a bus — live A/B generation (canonical or `__stg`).
fn post_meter_sink(bus: &str) -> String {
    buschain_engine::live_post_name(bus)
}

/// In-app plugin editor (never a separate OS/viewport window).
#[derive(Debug, Clone)]
pub struct PluginWindow {
    pub track_id: Uuid,
    pub slot_id: Uuid,
    pub fullscreen: bool,
    /// Bumps egui window id so each open uses the full preferred size (no tiny restore).
    pub open_gen: u64,
    /// When false, host chrome stays slim and prefers the out-of-process native GUI
    /// (VST3). Converted egui params are opt-in via this flag.
    pub show_converted_ui: bool,
    /// Surface helper has received `E float` for this window.
    pub surface_editor_attached: bool,
}

pub struct AppState {
    pub theme: SpectrumTheme,
    pub session: Session,
    pub snapshot: PwSnapshot,
    pub plugins: PluginHost,
    pub worker: AudioWorker,
    pub meters: MeterHub,
    pub tab: usize,
    /// Settings category: Appearance / Audio / Plugins / Session / Advanced.
    pub settings_tab: usize,
    pub selected_track: Option<Uuid>,
    /// Selected insert on the selected track (rack highlight).
    pub selected_plugin_slot: Option<Uuid>,
    pub status: String,
    pub dirty: bool,
    /// Mixer channel-rack SidePanel width.
    pub rack_width: f32,
    /// Collapsible bottom analyzer under mixer strips.
    pub mixer_analyzer_open: bool,
    /// Open analyzer panel height (drag-resizable).
    pub mixer_analyzer_height: f32,
    /// Open plugin editors (in-app `egui::Window`s). Ordered oldest → newest;
    /// Esc closes the last entry (latest selected / focused).
    pub plugin_windows: Vec<PluginWindow>,
    /// Monotonic counter so each floating plugin open gets a fresh egui size.
    plugin_win_gen: u64,
    /// Cached freedesktop / theme app icons (`icon_name` → texture).
    pub app_icon_cache: HashMap<String, TextureHandle>,
    /// Reveal internal loopbacks / helper sinks in Playback / Recording / Output.
    pub show_hidden_playback: bool,
    pub show_hidden_recording: bool,
    pub show_hidden_output: bool,
    /// Last probed Master HW capabilities (UI).
    pub device_caps: buschain_engine::DeviceCaps,
    /// Per-device caps for Output/Input clock panels — filled lazily, never every frame.
    pub device_caps_by_name: HashMap<String, (Instant, buschain_engine::DeviceCaps)>,
    /// At most one expensive caps probe per UI tick (avoids N×pw-dump hitches).
    caps_probe_budget: u8,
    /// Deferred track removal — applied after the mixer strip pass (avoids index panic).
    pending_remove_track: Option<Uuid>,
    /// Pending single-track fader push.
    pending_level_track: Option<Uuid>,
    /// Pending full mute/solo recount.
    levels_pending_full: bool,
    levels_last_sent: Option<Instant>,
    hotplug_deadline: Option<Instant>,
    /// Tracks pending surgical FX rewire (never escalates to session routes).
    fx_dirty_tracks: HashSet<Uuid>,
    /// Live param push (knobs) — drag coalesce; discrete ops push immediately.
    params_deadline: Option<Instant>,
    params_track: Option<Uuid>,
    /// After a failed Props push, allow deferred retries (bounded).
    params_retry_armed: bool,
    /// Count of wet-race Props retries for the current params_track.
    params_retry_count: u8,
    meter_targets_sig: u64,
    /// Second meter rebind after bus migrate (posts may appear a beat late).
    meter_rebind_deadline: Option<Instant>,
    reattach_attempted: bool,
    /// Settings → Session "Save as…" draft.
    pub session_save_as_open: bool,
    pub session_save_as_name: String,
    /// Compact overlay window (`buschain-control --popup` process).
    pub popup_mode: bool,
    /// In-process QS-like mixer popup (tray left-click while UI is running).
    pub mixer_popup_open: bool,
    /// Popup tab: 0 Playback · 1 Tracks · 2 Output · 3 Input.
    pub mixer_popup_tab: u8,
    /// Set by Settings → Quit (window close only hides).
    pub request_quit: bool,
    /// Tray / IPC asked to show the main window.
    pub request_show: bool,
    /// Tray asked to withdraw the main window.
    pub request_hide: bool,
    /// UI visualization live (meters / spectrum). False while withdrawn or headless.
    pub viz_live: bool,
    /// Cold ApplySession / ArmSession in flight — show loading chrome until done.
    pub graph_loading: bool,
    /// Live MIDI device list + activity meters.
    pub midi_snapshot: buschain_engine::MidiSnapshot,
    /// Selected device for route matrix / learn filter.
    pub midi_selected_device: Option<String>,
    /// Learn mode status line.
    pub midi_learn: Option<MidiLearnState>,
}

#[derive(Debug, Clone)]
pub struct MidiLearnState {
    pub hint: String,
}

impl AppState {
    pub fn new() -> Self {
        let boot = std::time::Instant::now();
        let lat = matches!(
            std::env::var("BUSCHAIN_CONTROL_LAT_TRACE").as_deref(),
            Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
        );
        let mut session = Session::load();
        // Never auto-wire the graph on launch — that was stacking loopbacks / crackling.
        session.autostart_graph = false;
        if lat {
            eprintln!(
                "[buschain-lat] session_load {}ms",
                boot.elapsed().as_millis()
            );
        }
        let scan_t = std::time::Instant::now();
        let plugins = PluginHost::new(
            &session.ladspa_paths,
            &session.lv2_paths,
            &session.clap_paths,
            session.vst3_enabled,
        );
        if lat {
            eprintln!(
                "[buschain-lat] plugin_rescan {}ms",
                scan_t.elapsed().as_millis()
            );
        }
        let worker = AudioWorker::spawn();
        // Embed ctl/waybar IPC into this process (same worker — no thin-client hop).
        if !worker.via_daemon {
            if let Err(e) = crate::daemon::start_embedded(worker.clone_sender()) {
                eprintln!("buschain-control: embedded IPC: {e:#}");
            }
            if lat {
                eprintln!(
                    "[buschain-lat] sock_ready {}ms",
                    boot.elapsed().as_millis()
                );
            }
        }
        let meters = MeterHub::spawn();
        let selected = session
            .master_id()
            .or_else(|| session.tracks.first().map(|t| t.id));
        let theme = SpectrumTheme::from_session_rgb(session.accent_rgb);
        let via_daemon = worker.via_daemon;
        let mut state = Self {
            theme,
            session,
            snapshot: PwSnapshot::default(),
            plugins,
            worker,
            meters,
            tab: 0,
            settings_tab: 0,
            selected_track: selected,
            selected_plugin_slot: None,
            status: "BusChain Control ready — buses persist across restart".into(),
            dirty: false,
            rack_width: 360.0,
            mixer_analyzer_open: true,
            mixer_analyzer_height: 172.0,
            plugin_windows: Vec::new(),
            plugin_win_gen: 0,
            app_icon_cache: HashMap::new(),
            show_hidden_playback: false,
            show_hidden_recording: false,
            show_hidden_output: false,
            device_caps: crate::audio::pipeline::DeviceCaps::default(),
            device_caps_by_name: HashMap::new(),
            caps_probe_budget: 1,
            pending_remove_track: None,
            pending_level_track: None,
            levels_pending_full: false,
            levels_last_sent: None,
            hotplug_deadline: None,
            fx_dirty_tracks: HashSet::new(),
            params_deadline: None,
            params_track: None,
            params_retry_armed: false,
            params_retry_count: 0,
            meter_targets_sig: 0,
            meter_rebind_deadline: None,
            reattach_attempted: false,
            session_save_as_open: false,
            session_save_as_name: String::new(),
            popup_mode: false,
            mixer_popup_open: false,
            mixer_popup_tab: 0,
            request_quit: false,
            request_show: false,
            request_hide: false,
            viz_live: true,
            graph_loading: !via_daemon,
            midi_snapshot: buschain_engine::MidiSnapshot::default(),
            midi_selected_device: None,
            midi_learn: None,
        };
        state.refresh_performance_from_device();
        if via_daemon {
            state.status = "Connected to external daemon (--daemon-client)".into();
        } else {
            state.worker.send(Command::ApplySession(state.session.clone()));
            state.status = "Loading audio graph — FX racks starting (this can take a few seconds)…".into();
            state.worker.send(Command::ApplyMidiConfig(state.session.clone()));
            state.worker.send(Command::RefreshMidi);
        }
        state
    }

    pub fn rebuild_plugins(&mut self) {
        self.plugins = PluginHost::new(
            &self.session.ladspa_paths,
            &self.session.lv2_paths,
            &self.session.clap_paths,
            self.session.vst3_enabled,
        );
    }

    pub fn graph_is_live(&self) -> bool {
        // Master null-sink present ⇒ graph is up. Do not require session
        // `sink_name` — that field can lag / stay None while audio is playing,
        // and treating that as "cold" made +Track escalate to Full Apply which
        // cold-disarmed Master→HW (silence).
        self.snapshot
            .sinks
            .iter()
            .any(|s| s.name == "buschain_master")
    }

    /// Idle reconcile / recovery. Normal edits use [`Self::commit`] and stay live.
    pub fn apply_graph(&mut self) {
        self.commit(LiveChange::Reconcile);
    }

    /// ERROR RECOVERY ONLY — not a normal edit path.
    pub fn teardown_graph(&mut self) {
        self.hotplug_deadline = None;
        self.fx_dirty_tracks.clear();
        self.params_deadline = None;
        self.params_track = None;
        self.pending_level_track = None;
        self.levels_pending_full = false;
        self.worker.send(Command::Teardown);
        for t in &mut self.session.tracks {
            t.sink_name = None;
        }
        self.meters.set_targets(vec![]);
        self.meter_targets_sig = 0;
        self.status = "Tear down (recovery) — unloading BusChain modules…".into();
    }

    /// **Live Graph Contract entry point.** All audio-affecting session mutations
    /// must go through here (see `audio/live.rs`). Auto-brings up a cold graph.
    pub fn commit(&mut self, change: LiveChange) {
        self.dirty = true;

        // Cold graph: only explicit Reconcile may Full Apply / ArmSession.
        // EnsureTrack / Route / VirtualInput / FxRewire stay surgical — escalating
        // them used to cold-disarm Master→HW (silence on +Track / mic assign).
        if change.needs_graph()
            && !self.graph_is_live()
            && matches!(change, LiveChange::Reconcile)
        {
            self.hotplug_deadline = None;
            self.fx_dirty_tracks.clear();
            self.params_deadline = None;
            self.params_track = None;
            self.pending_level_track = None;
            self.levels_pending_full = false;
            self.worker
                .send(Command::ApplySession(self.session.clone()));
            self.graph_loading = true;
            self.status = format!(
                "Loading audio graph ({}) — streams stay on buses…",
                change.label()
            );
            return;
        }

        match change {
            LiveChange::Level { track_id } => {
                self.pending_level_track = Some(track_id);
                self.flush_levels(false);
            }
            LiveChange::Levels => {
                self.levels_pending_full = true;
                self.flush_levels(false);
            }
            LiveChange::FxParams { track_id } => {
                self.params_track = Some(track_id);
                self.params_retry_armed = false;
                self.params_retry_count = 0;
                // Drag coalesce — discrete power uses flush_fx_params_now (immediate).
                let gap = crate::audio::adaptive::AdaptivePolicy::global()
                    .drag_coalesce_ms();
                self.params_deadline =
                    Some(Instant::now() + Duration::from_millis(gap));
            }
            LiveChange::FxRewire { track_id } => {
                self.fx_dirty_tracks.insert(track_id);
                let gap = crate::audio::adaptive::AdaptivePolicy::global()
                    .fx_coalesce_ms();
                self.hotplug_deadline =
                    Some(Instant::now() + Duration::from_millis(gap));
                self.status = "Live FX rewire — slots (streams untouched)…".into();
            }
            LiveChange::EnsureTrack { track_id } => {
                self.worker.send(Command::EnsureTrack {
                    session: self.session.clone(),
                    track_id,
                });
                self.status = "Live ensure track bus…".into();
            }
            LiveChange::VirtualInput { track_id } => {
                self.worker.send(Command::VirtualInput {
                    session: self.session.clone(),
                    track_id,
                });
                self.status = "Live virtual input…".into();
            }
            LiveChange::Capture { track_id } => {
                self.worker.send(Command::SyncCaptureDelta {
                    session: self.session.clone(),
                    track_id,
                });
                self.status = "Live capture — In hops…".into();
            }
            LiveChange::PlaceApp { track_id } => {
                // PlaceApp-only hot path (never SyncPlayback after every Add).
                // Unpin-all / EnsureTrack / idle reclaim still use SyncPlayback.
                if let Some(track) = self.session.tracks.iter().find(|t| t.id == track_id) {
                    let sink = track
                        .sink_name
                        .clone()
                        .unwrap_or_else(|| track.expected_sink_name());
                    if let Some(app_key) = track.assigned_playback.last().cloned() {
                        self.worker.send(Command::PlaceApp {
                            session: self.session.clone(),
                            app_key,
                            sink,
                        });
                    } else {
                        self.worker
                            .send(Command::SyncPlayback(self.session.clone()));
                    }
                }
                self.status = "Placing app streams…".into();
            }
            LiveChange::Route => {
                // Egress + listen + Master HW only — never capture purge.
                self.worker.send(Command::RewireSessionRoutes(
                    self.session.clone(),
                ));
                self.status = "Live routing — egress only…".into();
            }
            LiveChange::Reconcile => {
                self.hotplug_deadline = None;
                self.fx_dirty_tracks.clear();
                self.params_deadline = None;
                self.params_track = None;
                self.pending_level_track = None;
                self.levels_pending_full = false;
                self.worker
                    .send(Command::ApplySession(self.session.clone()));
                self.status =
                    "Reconcile graph (idle ensure — won’t move streams)…".into();
                self.dirty = false;
            }
        }
    }

    /// Resolve `(track_idx, insert_idx)` for an open plugin window key.
    pub fn find_insert(&self, track_id: Uuid, slot_id: Uuid) -> Option<(usize, usize)> {
        let ti = self.session.track_index(track_id)?;
        let ii = self.session.tracks[ti]
            .inserts
            .iter()
            .position(|p| p.slot_id == slot_id)?;
        Some((ti, ii))
    }

    /// Select an insert (rack highlight + drawer tab). Does not open a floating window.
    pub fn select_plugin(&mut self, track_id: Uuid, slot_id: Uuid) {
        self.selected_track = Some(track_id);
        self.selected_plugin_slot = Some(slot_id);
    }

    /// Select insert index `0..9` on the current track (Ctrl+1‥9). Expands drawer.
    pub fn select_plugin_by_index(&mut self, index: usize) -> bool {
        let Some(tid) = self.selected_track else {
            return false;
        };
        let Some(ti) = self.session.track_index(tid) else {
            return false;
        };
        let inserts = &self.session.tracks[ti].inserts;
        if index >= inserts.len() || index > 8 {
            return false;
        }
        let slot = inserts[index].slot_id;
        // Ctrl+1‥9 opens (or focuses) that insert's floating window.
        self.open_plugin_window(tid, slot);
        true
    }

    /// Recompute Balanced/Low/Stable from Master HW out probe (Custom keeps numbers).
    pub fn refresh_performance_from_device(&mut self) {
        use buschain_engine::{probe_master_hw, resolve_profile, AudioPreset};
        // One resolver for route + probe + UI (same as worker).
        if let Ok(hw) = crate::audio::graph::resolve_hardware_output(&self.session) {
            if self.session.master_output.as_deref() != Some(hw.as_str()) {
                self.session.master_output = Some(hw.clone());
            }
            if let Some(d) = self.snapshot.sinks.iter().find(|s| s.name == hw) {
                if !d.description.is_empty() {
                    self.session.master_output_desc = Some(d.description.clone());
                }
            }
        }
        let caps = probe_master_hw(self.session.master_output.as_deref());
        self.device_caps = caps.clone();
        let preset = self.session.performance.preset;
        if preset == AudioPreset::Custom {
            let soft = self.session.performance.soft_quantum;
            let rate = self.session.performance.sample_rate;
            let q = self.session.performance.quantum;
            let before_rate = rate;
            let before_q = q;
            self.session.performance = resolve_profile(
                AudioPreset::Custom,
                &caps,
                Some(rate),
                Some(q),
                soft,
            );
            let after = &self.session.performance;
            if after.device_limited
                || after.sample_rate != before_rate
                || after.quantum != before_q
            {
                self.status = format!(
                    "Custom clamped to {} Hz · q{} (device caps)",
                    after.sample_rate, after.quantum
                );
                self.dirty = true;
            } else if caps.preferred_rate > 0 && after.sample_rate != caps.preferred_rate {
                self.status = format!(
                    "Custom {} Hz · Apply to switch HW from {} Hz",
                    after.sample_rate, caps.preferred_rate
                );
            }
            return;
        }
        let soft = self.session.performance.soft_quantum;
        self.session.performance = resolve_profile(preset, &caps, None, None, soft);
        self.dirty = true;
    }

    /// Bind GraphClock from session performance and rewire (Config → Apply audio settings).
    pub fn apply_audio_clock(&mut self) {
        self.refresh_performance_from_device();
        // Keep per-device prefs in sync with Master HW out.
        if let Some(hw) = self.session.master_output.clone() {
            self.session.device_clocks.insert(
                hw,
                crate::session::DeviceClockConfig {
                    sample_rate: self.session.performance.sample_rate,
                    quantum: self.session.performance.quantum,
                    soft_quantum: self.session.performance.soft_quantum,
                },
            );
        }
        let _ = self.session.save();
        self.worker.send(Command::BindMasterClock(self.session.clone()));
        self.status = format!(
            "Clock {} Hz · q{} — binding…",
            self.session.performance.sample_rate, self.session.performance.quantum
        );
    }

    /// Cached caps for a HW sink/source (TTL). Safe to call from UI draw.
    pub fn caps_for_device(
        &mut self,
        name: &str,
        description: &str,
        is_source: bool,
    ) -> buschain_engine::DeviceCaps {
        const TTL: Duration = Duration::from_secs(10);
        if let Some((at, caps)) = self.device_caps_by_name.get(name) {
            if at.elapsed() < TTL || self.caps_probe_budget == 0 {
                return caps.clone(); // fresh, or stale-while-revalidate
            }
        } else if self.caps_probe_budget == 0 {
            // Cheap placeholder until a later frame can probe.
            let live = self
                .snapshot
                .sinks
                .iter()
                .chain(self.snapshot.sources.iter())
                .find(|d| d.name == name)
                .and_then(|d| d.sample_rate)
                .or_else(|| {
                    self.session
                        .device_clocks
                        .get(name)
                        .map(|c| c.sample_rate)
                })
                .unwrap_or(48_000);
            return buschain_engine::DeviceCaps {
                sink_name: name.to_string(),
                description: description.to_string(),
                rates: vec![live],
                preferred_rate: live,
                min_quantum: 64,
                max_quantum: 2048,
                preferred_quantum: 256,
            };
        }
        self.caps_probe_budget = self.caps_probe_budget.saturating_sub(1);
        let caps = buschain_engine::probe_named_device_caps(name, description, is_source);
        self.device_caps_by_name
            .insert(name.to_string(), (Instant::now(), caps.clone()));
        if !is_source && self.session.master_output.as_deref() == Some(name) {
            self.device_caps = caps.clone();
        }
        caps
    }

    /// Apply rate/quantum for one HW sink or source (Output/Input device panels).
    pub fn apply_device_clock(&mut self, device: &str, is_source: bool) {
        use buschain_engine::{resolve_profile, AudioPreset};
        let desc = self
            .snapshot
            .sinks
            .iter()
            .chain(self.snapshot.sources.iter())
            .find(|d| d.name == device)
            .map(|d| d.description.clone())
            .unwrap_or_else(|| device.to_string());
        let caps = buschain_engine::probe_named_device_caps(device, &desc, is_source);
        self.device_caps_by_name
            .insert(device.to_string(), (Instant::now(), caps.clone()));
        let entry = self
            .session
            .device_clocks
            .entry(device.to_string())
            .or_default();
        let soft = entry.soft_quantum;
        let resolved = resolve_profile(
            AudioPreset::Custom,
            &caps,
            Some(entry.sample_rate),
            Some(entry.quantum),
            soft,
        );
        entry.sample_rate = resolved.sample_rate;
        entry.quantum = resolved.quantum;
        entry.soft_quantum = soft;
        let sample_rate = entry.sample_rate;
        let quantum = entry.quantum;
        let soft_quantum = entry.soft_quantum;

        let bind_buschain = !is_source
            && self.session.master_output.as_deref() == Some(device);
        if bind_buschain {
            self.session.performance = resolved;
            self.session.performance.preset = AudioPreset::Custom;
        }
        let _ = self.session.save();
        self.worker.send(Command::BindDeviceClock {
            session: self.session.clone(),
            device: device.to_string(),
            sample_rate,
            quantum,
            soft_quantum,
            bind_buschain,
        });
        self.status = format!(
            "{} {} Hz · q{} — applying…",
            if bind_buschain {
                "Master HW + BusChain"
            } else {
                "Device"
            },
            sample_rate,
            quantum
        );
    }

    /// Formats that currently get a real out-of-process GUI via `buschain-plugin-ui`.
    pub fn insert_supports_native_editor(&self, track_id: Uuid, slot_id: Uuid) -> bool {
        use crate::audio::plugin::PluginFormat;
        self.session
            .tracks
            .iter()
            .find(|t| t.id == track_id)
            .and_then(|t| t.inserts.iter().find(|p| p.slot_id == slot_id))
            .is_some_and(|p| matches!(p.id.format, PluginFormat::Vst3))
    }

    /// Open a floating plugin window, or focus it if already open.
    /// Multiple editors may be open; this one becomes newest (Esc closes it first).
    /// VST3 defaults to native editor + slim host chrome (converted egui params opt-in).
    pub fn open_plugin_window(&mut self, track_id: Uuid, slot_id: Uuid) {
        self.select_plugin(track_id, slot_id);
        if let Some(idx) = self
            .plugin_windows
            .iter()
            .position(|w| w.track_id == track_id && w.slot_id == slot_id)
        {
            self.focus_plugin_window_at(idx);
            return;
        }
        let prefer_native = self.insert_supports_native_editor(track_id, slot_id);
        let next_gen = self.plugin_win_gen.wrapping_add(1);
        self.plugin_win_gen = next_gen;
        self.plugin_windows.push(PluginWindow {
            track_id,
            slot_id,
            fullscreen: false,
            open_gen: next_gen,
            show_converted_ui: !prefer_native,
            surface_editor_attached: false,
        });
        if prefer_native {
            self.open_native_editor(track_id, slot_id);
        }
    }

    /// Open native editor. VST3 promotes to `buschain-plugin-surface` (DSP+GUI, live meters).
    pub fn open_native_editor(&mut self, track_id: Uuid, slot_id: Uuid) {
        self.harvest_track_plugin_state(track_id);

        let Some(track) = self.session.tracks.iter().find(|t| t.id == track_id) else {
            return;
        };
        let Some(plug) = track.inserts.iter().find(|p| p.slot_id == slot_id) else {
            return;
        };
        use crate::audio::plugin::PluginFormat;
        let format = match plug.id.format {
            PluginFormat::Clap => "clap",
            PluginFormat::Vst3 => "vst3",
            PluginFormat::Lv2 => "lv2",
            PluginFormat::Ladspa => {
                self.status = "LADSPA has no native editor — use egui params".into();
                return;
            }
        };
        let bus = track.expected_sink_name();
        let path = plug.id.id.clone();
        let state_blob = plug.state_blob.clone();
        let plugin_key = plug.id.id.clone();

        // VST3 surface path: same-instance DSP+GUI (meters work).
        if format == "vst3" && buschain_engine::surface_enabled() {
            // Already promoted and helper is up — reopen float only (never rewire/truncate SHM).
            if buschain_engine::slot_wants_surface(slot_id)
                && buschain_engine::surface_ctrl_ready(slot_id)
            {
                if let Some(w) = self
                    .plugin_windows
                    .iter_mut()
                    .find(|w| w.slot_id == slot_id)
                {
                    w.surface_editor_attached = false;
                }
                if buschain_engine::surface_send_ctrl(slot_id, "E float") {
                    if let Some(w) = self
                        .plugin_windows
                        .iter_mut()
                        .find(|w| w.slot_id == slot_id)
                    {
                        w.surface_editor_attached = true;
                    }
                    crate::hyprland_float::request_float_vst3_surfaces();
                    self.status = "Native surface — floating editor".into();
                } else {
                    self.status = "Native surface — helper busy, retry…".into();
                }
                return;
            }
            buschain_engine::ensure_editor_ipc(&bus, slot_id);
            buschain_engine::mark_surface_slot(slot_id);
            if let Some(w) = self
                .plugin_windows
                .iter_mut()
                .find(|w| w.slot_id == slot_id)
            {
                w.surface_editor_attached = false;
            }
            self.schedule_fx_rewire(track_id);
            // Embed/float attach happens from plugin_windows once the surface ctrl is up.
            self.status = "Native surface — promoting VST3 (DSP+GUI)…".into();
            return;
        }

        buschain_engine::request_open_editor(&buschain_engine::OpenEditorRequest {
            bus,
            slot_id,
            plugin_key,
            plugin_path: path,
            format: format.into(),
            socket_path: String::new(),
            state_blob,
            parent_xid: None,
            embed: false,
        });
        self.status = format!("Native editor — {format}");
    }

    /// Open the floating native editor once the surface helper ctrl socket is ready.
    pub fn try_attach_surface_editor(&mut self, slot_id: Uuid) {
        if !buschain_engine::slot_wants_surface(slot_id) {
            return;
        }
        if self
            .plugin_windows
            .iter()
            .any(|w| w.slot_id == slot_id && w.surface_editor_attached)
        {
            return;
        }
        if buschain_engine::surface_send_ctrl(slot_id, "E float") {
            if let Some(w) = self
                .plugin_windows
                .iter_mut()
                .find(|w| w.slot_id == slot_id)
            {
                w.surface_editor_attached = true;
            }
            crate::hyprland_float::request_float_vst3_surfaces();
            self.status = "Native surface — floating editor".into();
        }
    }

    pub fn destroy_surface_editor(&mut self, slot_id: Uuid) {
        // Detach GUI before killing the helper so plugins don't paint into a dead drawable.
        let _ = buschain_engine::surface_send_ctrl(slot_id, "E close");
        let _ = buschain_engine::surface_send_ctrl(slot_id, "Q");
        buschain_engine::surface_forget_ctrl(slot_id);
    }

    /// Mirror live host processor state into the session (params + state_blob).
    /// Call before Props flush / rewire so session defaults don't stomp editor edits.
    pub fn harvest_track_plugin_state(&mut self, track_id: Uuid) {
        let Some(track) = self.session.tracks.iter().find(|t| t.id == track_id) else {
            return;
        };
        let bus = track.expected_sink_name();
        let snaps = buschain_engine::harvest_host_slot_states(&bus);
        if snaps.is_empty() {
            return;
        }
        let Some(track) = self.session.tracks.iter_mut().find(|t| t.id == track_id) else {
            return;
        };
        for snap in snaps {
            let Some(plug) = track
                .inserts
                .iter_mut()
                .find(|p| p.slot_id == snap.slot_id)
            else {
                continue;
            };
            if let Some(blob) = snap.state_blob {
                if !blob.is_empty() {
                    plug.state_blob = Some(blob);
                }
            }
            for (name, val) in snap.controls {
                let lname = name.to_ascii_lowercase();
                if lname == "bypass" || lname == "enable" || name == "Mix" {
                    continue;
                }
                if let Some((_, v)) = plug
                    .params
                    .iter_mut()
                    .find(|(k, _)| k.eq_ignore_ascii_case(&name))
                {
                    *v = val;
                } else {
                    plug.params.push((name, val));
                }
            }
        }
        self.dirty = true;
    }

    /// Apply out-of-process editor param events into the session param map.
    fn drain_editor_params_into_session(&mut self) {
        for slot_id in buschain_engine::drain_editor_closed_events() {
            if let Some(w) = self
                .plugin_windows
                .iter_mut()
                .find(|w| w.slot_id == slot_id)
            {
                w.surface_editor_attached = false;
            }
            self.status = "Native editor closed — DSP kept running".into();
        }
        let events = buschain_engine::drain_editor_param_events();
        if events.is_empty() {
            return;
        }
        for ev in events {
            for track in &mut self.session.tracks {
                let Some(plug) = track
                    .inserts
                    .iter_mut()
                    .find(|p| p.slot_id == ev.slot_id)
                else {
                    continue;
                };
                // Probe order matches editor / host control indices.
                plug.ensure_params();
                let idx = ev.control_index as usize;
                if let Some((_, v)) = plug.params.get_mut(idx) {
                    *v = ev.value;
                    self.dirty = true;
                }
                break;
            }
        }
    }

    /// Mark an open editor as latest-selected (drawn on top; Esc closes it first).
    pub fn focus_plugin_window(&mut self, track_id: Uuid, slot_id: Uuid) {
        if let Some(idx) = self
            .plugin_windows
            .iter()
            .position(|w| w.track_id == track_id && w.slot_id == slot_id)
        {
            self.focus_plugin_window_at(idx);
            self.select_plugin(track_id, slot_id);
        }
    }

    fn focus_plugin_window_at(&mut self, idx: usize) {
        if idx + 1 == self.plugin_windows.len() {
            return;
        }
        let w = self.plugin_windows.remove(idx);
        self.plugin_windows.push(w);
    }

    pub fn close_plugin_window(&mut self, track_id: Uuid, slot_id: Uuid) {
        self.plugin_windows
            .retain(|w| !(w.track_id == track_id && w.slot_id == slot_id));
        let was_surface = buschain_engine::slot_wants_surface(slot_id);
        // Mirror params before killing the surface helper.
        if was_surface {
            self.harvest_track_plugin_state(track_id);
        }
        self.destroy_surface_editor(slot_id);
        buschain_engine::clear_surface_slot(slot_id);
        buschain_engine::close_editor(slot_id);
        if was_surface {
            // Demote back to in-process VST3.
            self.schedule_fx_rewire(track_id);
        }
    }

    pub fn toggle_plugin_fullscreen(&mut self, track_id: Uuid, slot_id: Uuid) {
        if let Some(w) = self
            .plugin_windows
            .iter_mut()
            .find(|w| w.track_id == track_id && w.slot_id == slot_id)
        {
            w.fullscreen = !w.fullscreen;
        }
    }

    /// Keep `selected_plugin_slot` valid for the current track.
    pub fn sync_selected_plugin(&mut self) {
        let Some(tid) = self.selected_track else {
            self.selected_plugin_slot = None;
            return;
        };
        let Some(ti) = self.session.track_index(tid) else {
            self.selected_plugin_slot = None;
            return;
        };
        let inserts = &self.session.tracks[ti].inserts;
        if inserts.is_empty() {
            self.selected_plugin_slot = None;
            return;
        }
        let still_ok = self
            .selected_plugin_slot
            .map(|s| inserts.iter().any(|p| p.slot_id == s))
            .unwrap_or(false);
        if !still_ok {
            self.selected_plugin_slot = Some(inserts[0].slot_id);
        }
    }

    /// Queue track removal after the current UI pass (safe during strip iteration).
    pub fn request_remove_track(&mut self, id: Uuid) {
        if self
            .session
            .tracks
            .iter()
            .any(|t| t.id == id && t.kind.is_master())
        {
            return;
        }
        self.pending_remove_track = Some(id);
    }

    pub fn apply_pending_track_remove(&mut self) {
        let Some(id) = self.pending_remove_track.take() else {
            return;
        };
        if self
            .session
            .tracks
            .iter()
            .any(|t| t.id == id && t.kind.is_master())
        {
            return;
        }
        let removed_bus = self
            .session
            .tracks
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.expected_sink_name());
        self.session.tracks.retain(|t| t.id != id);
        for t in &mut self.session.tracks {
            t.output_targets.retain(|x| *x != id);
        }
        self.plugin_windows.retain(|w| w.track_id != id);
        if self.selected_track == Some(id) {
            self.selected_track = self.session.master_id();
        }
        self.dirty = true;
        // Surgical prune only — never reload every track's FX (that caused multi-second farts).
        if self.graph_is_live() {
            if let Some(bus) = removed_bus {
                self.worker.send(Command::PruneTrack {
                    session: self.session.clone(),
                    removed_bus: bus,
                });
                self.status = "Removing track bus (others untouched)…".into();
            }
        }
        let _ = self.session.save();
    }

    /// New track — create its bus (auto bring-up if cold).
    pub fn ensure_new_track(&mut self, track_id: Uuid) {
        self.commit(LiveChange::EnsureTrack { track_id });
    }

    /// Idle / withdraw — pause Pulse meters and clear host FFT watches.
    pub fn sleep_visualization(&mut self) {
        self.viz_live = false;
        self.meters.set_paused(true);
        crate::audio::engine_handle::host_spectrum_clear_watches();
    }

    /// Show window — resume meters and retarget taps immediately.
    pub fn wake_visualization(&mut self) {
        self.viz_live = true;
        self.meters.set_paused(false);
        self.meter_targets_sig = 0;
        self.sync_meter_targets();
        self.meters.force_rebuild();
    }

    fn sync_meter_targets(&mut self) {
        if !self.viz_live {
            return;
        }
        // Pulse only for buses without a live in-process host (dry / fallback).
        let rate = self.session.performance.sample_rate.max(8_000);
        let mut taps: Vec<MeterTap> = Vec::new();
        let mut sig: u64 = 0;
        for t in &self.session.tracks {
            let bus = t
                .sink_name
                .clone()
                .unwrap_or_else(|| t.expected_sink_name());
            if crate::audio::engine_handle::host_is_live(&bus) {
                // Host peaks own this strip — no Pulse monitor stream.
                sig = sig.wrapping_mul(16777619) ^ 0x484F_5354u64; // "HOST"
                for b in bus.as_bytes() {
                    sig = sig.wrapping_mul(16777619) ^ (*b as u64);
                }
                continue;
            }
            let post = post_meter_sink(&bus);
            let post_live = self.snapshot.sinks.iter().any(|s| s.name == post)
                || crate::audio::engine_handle::chain_is_wet_cached(&bus);
            for b in bus.as_bytes() {
                sig = sig.wrapping_mul(16777619) ^ (*b as u64);
            }
            sig = sig.wrapping_mul(16777619) ^ (rate as u64);
            taps.push(MeterTap {
                key: bus.clone(),
                tap: bus.clone(),
                sample_rate: rate,
            });
            if !t.inserts.is_empty() && post_live && post != bus {
                for b in post.as_bytes() {
                    sig = sig.wrapping_mul(16777619) ^ (*b as u64);
                }
                taps.push(MeterTap {
                    key: bus,
                    tap: post,
                    sample_rate: rate,
                });
            }
        }
        sig = sig.wrapping_mul(16777619) ^ (taps.len() as u64);
        if sig != self.meter_targets_sig {
            self.meter_targets_sig = sig;
            self.meters.set_targets(taps);
        }
    }

    /// After clock bind / bus migrate: names stay the same but PW indices change.
    fn force_meter_rebind(&mut self, schedule_followup: bool) {
        self.meter_targets_sig = 0;
        if !self.viz_live {
            if schedule_followup {
                self.meter_rebind_deadline = Some(Instant::now() + Duration::from_millis(450));
            }
            return;
        }
        self.sync_meter_targets();
        self.meters.force_rebuild();
        if schedule_followup {
            // Posts/FX may finish spawning a moment after SessionApplied.
            self.meter_rebind_deadline = Some(Instant::now() + Duration::from_millis(450));
        }
    }

    /// Fast fader path — one sink, coalesced in the worker.
    pub fn schedule_track_level(&mut self, track_id: Uuid) {
        self.commit(LiveChange::Level { track_id });
    }

    /// Mute/solo changed — recount all track audibility.
    pub fn schedule_levels(&mut self) {
        self.commit(LiveChange::Levels);
    }

    fn flush_levels(&mut self, force: bool) {
        if self.pending_level_track.is_none() && !self.levels_pending_full {
            return;
        }
        let now = Instant::now();
        // ~120 Hz cap; keep pending across frames so drags stay continuous.
        let min_gap = Duration::from_millis(
            crate::audio::adaptive::AdaptivePolicy::global().drag_coalesce_ms(),
        );
        let ready = force
            || self
                .levels_last_sent
                .map(|t| now.duration_since(t) >= min_gap)
                .unwrap_or(true);
        if !ready {
            return;
        }
        self.levels_last_sent = Some(now);

        if self.levels_pending_full {
            self.levels_pending_full = false;
            self.pending_level_track = None;
            // Per-track SetTrackLevel (native gate) — never ApplyLevels N× list_sinks.
            let any_solo = self
                .session
                .tracks
                .iter()
                .any(|t| t.solo && !t.kind.is_master());
            for t in &self.session.tracks {
                crate::daemon::push_track_mixer_to_daemon(t.id, t.gain_db, t.mute);
                let muted =
                    t.mute || (any_solo && !t.solo && !t.kind.is_master());
                self.worker.send(Command::SetTrackLevel {
                    sink: t.expected_sink_name(),
                    gain_db: t.gain_db,
                    muted,
                });
            }
            return;
        }

        // Don't take() — next frame may send a newer gain while this one is in flight.
        if let Some(track_id) = self.pending_level_track {
            let any_solo = self
                .session
                .tracks
                .iter()
                .any(|t| t.solo && !t.kind.is_master());
            if let Some(track) = self.session.tracks.iter().find(|t| t.id == track_id) {
                // Always address the deterministic bus — never skip when sink_name is None.
                let sink = track.expected_sink_name();
                let muted =
                    track.mute || (any_solo && !track.solo && !track.kind.is_master());
                crate::daemon::push_track_mixer_to_daemon(track_id, track.gain_db, track.mute);
                self.worker.send(Command::SetTrackLevel {
                    sink,
                    gain_db: track.gain_db,
                    muted,
                });
            }
            // Clear only after a successful schedule; a new drag re-sets it.
            self.pending_level_track = None;
        }
    }

    /// Routing / multi-track structural — live hotplug (auto bring-up if cold).
    pub fn schedule_hotplug(&mut self) {
        self.commit(LiveChange::Route);
    }

    /// Insert add/remove/reorder — per-slot spawn/stop + exclusive rewire.
    pub fn schedule_fx_rewire(&mut self, track_id: Uuid) {
        self.harvest_track_plugin_state(track_id);
        self.commit(LiveChange::FxRewire { track_id });
    }

    /// Legacy name — same as [`Self::schedule_fx_rewire`].
    pub fn schedule_hotplug_track(&mut self, track_id: Uuid) {
        self.schedule_fx_rewire(track_id);
    }

    /// Knobs / mix / insert power — graph-free Props push.
    ///
    /// Do **not** harvest here: egui knobs already wrote the session and harvesting
    /// would overwrite them with stale live values before the Props push.
    pub fn schedule_fx_params(&mut self, track_id: Uuid) {
        self.commit(LiveChange::FxParams { track_id });
    }

    /// Discrete power toggle — flush Props immediately (no coalesce wait).
    ///
    /// Pushes the host control queue from the UI thread first so power does not
    /// wait behind worker idle reconcile / placements (those can take seconds
    /// while `WorkerPushFx` still later reports 0ms). Worker mirrors session.
    pub fn flush_fx_params_now(&mut self, track_id: Uuid) {
        self.params_track = Some(track_id);
        self.params_retry_armed = false;
        self.params_retry_count = 0;
        self.params_deadline = None;
        self.dirty = true;
        if let Some((bus, inserts)) =
            crate::audio::insert_map::ladspa_slots_for(&self.session, track_id)
        {
            let span = buschain_engine::fx_trace::span("UiPushFx");
            match buschain_engine::host::registry::push_host_controls(&bus, &inserts) {
                Ok(()) => span.end_ok(),
                Err(e) => span.end(format!("defer {e:#}")),
            }
            self.worker.send(Command::PushFxControls { bus, inserts });
        } else {
            self.worker.send(Command::PushFxParams {
                session: self.session.clone(),
                track_id,
            });
        }
    }

    pub fn mark_routing_dirty(&mut self) {
        self.commit(LiveChange::Route);
    }

    /// In-rack add / mute-row / remove — capture hops only (never Route).
    pub fn mark_capture_dirty(&mut self, track_id: Uuid) {
        self.commit(LiveChange::Capture { track_id });
    }

    fn adopt_session(&mut self, session: Session) {
        self.session = session;
        if self.session.sandbox_untrusted {
            std::env::set_var("BUSCHAIN_SANDBOX_ALL", "1");
        } else {
            std::env::remove_var("BUSCHAIN_SANDBOX_ALL");
        }
        self.theme = SpectrumTheme::from_session_rgb(self.session.accent_rgb);
        self.rebuild_plugins();
        self.selected_track = self
            .session
            .master_id()
            .or_else(|| self.session.tracks.first().map(|t| t.id));
        self.selected_plugin_slot = self
            .selected_track
            .and_then(|tid| {
                self.session
                    .tracks
                    .iter()
                    .find(|t| t.id == tid)
                    .and_then(|t| t.inserts.first().map(|p| p.slot_id))
            });
    }

    pub fn clear_master_fx(&mut self) {
        let mid = self.session.master_id();
        if let Some(m) = self.session.tracks.iter_mut().find(|t| t.kind.is_master()) {
            m.inserts.clear();
        }
        if let Some(track_id) = mid {
            self.commit(LiveChange::FxRewire { track_id });
        }
        self.status = "Cleared Master FX — Save to persist".into();
    }

    pub fn new_session(&mut self) {
        let mut session = Session::default();
        session.name = "New session".into();
        session.slug = "untitled".into();
        self.apply_resolved_session(&mut session);
        self.adopt_session(session);
        self.worker
            .send(Command::ApplySession(self.session.clone()));
        self.status = "New session — Save as… to keep it".into();
        self.dirty = true;
    }

    pub fn save_session(&mut self) {
        if self.worker.via_daemon {
            use crate::ipc::{Client, Request, Response};
            // Push UI session into daemon, then persist.
            match Client::call(&Request::Exec {
                cmd: Command::ApplySession(self.session.clone()),
            }) {
                Ok(Response::Ok { session: Some(s), .. }) => {
                    self.adopt_session(s);
                }
                Ok(Response::Err { error }) => {
                    self.status = format!("Save failed: {error}");
                    return;
                }
                Err(e) => {
                    self.status = format!("Save failed: {e:#}");
                    return;
                }
                _ => {}
            }
            match Client::call(&Request::SessionSave) {
                Ok(Response::Ok { message, .. }) => {
                    self.status = if message.is_empty() {
                        format!("Saved session «{}»", self.session.name)
                    } else {
                        message
                    };
                    self.dirty = false;
                }
                Ok(Response::Mixer { .. }) => {}
                Ok(Response::Err { error }) => self.status = format!("Save failed: {error}"),
                Err(e) => self.status = format!("Save failed: {e:#}"),
            }
            return;
        }
        match self.session.save() {
            Ok(()) => {
                self.status = format!(
                    "Saved session «{}» ({})",
                    self.session.name,
                    Session::path().display()
                );
                self.dirty = false;
            }
            Err(e) => self.status = format!("Save failed: {e:#}"),
        }
    }

    pub fn save_session_as(&mut self, name: &str) {
        if self.worker.via_daemon {
            use crate::ipc::{Client, Request, Response};
            let _ = Client::call(&Request::Exec {
                cmd: Command::ApplySession(self.session.clone()),
            });
            match Client::call(&Request::SessionSaveAs {
                name: name.to_string(),
            }) {
                Ok(Response::Ok {
                    session: Some(s),
                    message,
                    ..
                }) => {
                    self.adopt_session(s);
                    self.status = if message.is_empty() {
                        format!("Saved session «{}»", self.session.name)
                    } else {
                        message
                    };
                    self.dirty = false;
                }
                Ok(Response::Ok { message, .. }) => {
                    self.status = message;
                    self.dirty = false;
                }
                Ok(Response::Mixer { .. }) => {}
                Ok(Response::Err { error }) => self.status = format!("Save as failed: {error}"),
                Err(e) => self.status = format!("Save as failed: {e:#}"),
            }
            return;
        }
        let slug = crate::session::store::slugify(name);
        match crate::session::store::save_session_as(&mut self.session, &slug, Some(name)) {
            Ok(()) => {
                self.status = format!("Saved session «{}»", self.session.name);
                self.dirty = false;
            }
            Err(e) => self.status = format!("Save as failed: {e:#}"),
        }
    }

    pub fn load_session_slug(&mut self, slug: &str) {
        if self.worker.via_daemon {
            use crate::ipc::{Client, Request, Response};
            match Client::call(&Request::SessionLoad {
                slug: slug.to_string(),
            }) {
                Ok(Response::Ok {
                    session: Some(s),
                    message,
                    ..
                }) => {
                    self.adopt_session(s);
                    self.status = if message.is_empty() {
                        format!("Loaded session «{}»", self.session.name)
                    } else {
                        message
                    };
                    self.dirty = false;
                    self.force_meter_rebind(true);
                }
                Ok(Response::Err { error }) => self.status = format!("Load failed: {error}"),
                Err(e) => self.status = format!("Load failed: {e:#}"),
                _ => self.status = "Load failed: no session in response".into(),
            }
            return;
        }
        match crate::session::store::load_slug(slug) {
            Ok(mut session) => {
                let _ = crate::session::store::write_active_slug(slug);
                self.apply_resolved_session(&mut session);
                self.adopt_session(session);
                self.worker
                    .send(Command::ApplySession(self.session.clone()));
                self.status = format!("Loaded session «{}»", self.session.name);
                self.dirty = false;
            }
            Err(e) => self.status = format!("Load failed: {e:#}"),
        }
    }

    pub fn delete_session_slug(&mut self, slug: &str) {
        if slug == self.session.slug {
            // Switch away first if possible.
            if let Ok(list) = crate::session::list_sessions() {
                if let Some(other) = list.iter().find(|m| m.slug != slug) {
                    self.load_session_slug(&other.slug);
                }
            }
        }
        match crate::session::store::delete_slug(slug) {
            Ok(()) => self.status = format!("Deleted session {slug}"),
            Err(e) => self.status = format!("Delete failed: {e:#}"),
        }
    }

    fn apply_resolved_session(&mut self, session: &mut Session) {
        let sinks: Vec<_> = self
            .snapshot
            .sinks
            .iter()
            .map(|s| (s.name.clone(), s.description.clone()))
            .collect();
        let sources: Vec<_> = self
            .snapshot
            .sources
            .iter()
            .map(|s| (s.name.clone(), s.description.clone()))
            .collect();
        let report = crate::session::resolve_devices(session, &sinks, &sources);
        if !report.messages.is_empty() {
            self.status = report.join();
        }
    }

    /// Keep runtime `sink_name` in sync whenever PW lists our buses (every snapshot).
    fn sync_bus_names_from_snapshot(&mut self) {
        let mut changed = false;
        for track in &mut self.session.tracks {
            let name = track.expected_sink_name();
            if self.snapshot.sinks.iter().any(|s| s.name == name) {
                if track.sink_name.as_deref() != Some(name.as_str()) {
                    track.sink_name = Some(name);
                    changed = true;
                }
            }
        }
        if changed {
            self.meter_targets_sig = 0;
        }
    }

    fn try_reattach_from_snapshot(&mut self) {
        if self.reattach_attempted {
            return;
        }
        if self.snapshot.sinks.is_empty() && self.snapshot.status.contains("sinks:") {
            return; // wait for a real snapshot
        }
        self.reattach_attempted = true;
        // Soft-bind missing HW / mics against the first real snapshot.
        {
            let sinks: Vec<_> = self
                .snapshot
                .sinks
                .iter()
                .map(|s| (s.name.clone(), s.description.clone()))
                .collect();
            let sources: Vec<_> = self
                .snapshot
                .sources
                .iter()
                .map(|s| (s.name.clone(), s.description.clone()))
                .collect();
            let report = crate::session::resolve_devices(&mut self.session, &sinks, &sources);
            if !report.messages.is_empty() {
                self.status = report.join();
            }
        }
        let n = self
            .session
            .tracks
            .iter()
            .filter(|t| t.sink_name.is_some())
            .count();
        if n > 0 {
            self.status = format!(
                "Reattached {n} existing BusChain bus(es) — audio left running from last session"
            );
            self.meter_targets_sig = 0;
            self.sync_meter_targets();
            self.schedule_levels();
        }
    }

    pub fn tick(&mut self) {
        self.apply_pending_track_remove();
        // Idle viz: skip expensive caps probes — audio cmds + events only.
        if self.viz_live {
            self.caps_probe_budget = 1;
        } else {
            self.caps_probe_budget = 0;
        }
        self.drain_editor_params_into_session();
        // Quickshell / ctl track vol → visual faders + dB readouts.
        for p in crate::daemon::take_track_mixer_ui_patches() {
            if let Some(t) = self.session.tracks.iter_mut().find(|t| t.id == p.track_id) {
                t.gain_db = p.gain_db;
                t.mute = p.mute;
            }
        }

        for ev in self.worker.poll() {
            match ev {
                Event::Snapshot(s) => {
                    if !self.status.starts_with("Applied")
                        && !self.status.starts_with("Saved")
                        && !self.status.starts_with("Moved")
                        && !self.status.starts_with("Tore down")
                        && !self.status.starts_with("Graph")
                        && !self.status.starts_with("Applying")
                        && !self.status.starts_with("Tearing")
                        && !self.status.starts_with("Hotplug")
                        && !self.status.starts_with("Live params")
                        && !self.status.starts_with("Plugin ")
                        && !self.status.starts_with("Reattached")
                        && !self.status.starts_with("Worker ready")
                        && !self.status.starts_with("apply:")
                        && !self.status.starts_with("teardown:")
                    {
                        self.status = s.status.clone();
                    }
                    self.snapshot = s;
                    self.sync_bus_names_from_snapshot();
                    self.try_reattach_from_snapshot();
                }
                Event::SinkInputs(inputs) => {
                    self.snapshot.sink_inputs = inputs;
                }
                Event::Status(s) => {
                    let skipped = s.starts_with("Live params skipped");
                    let live_ok = s.starts_with("Live params →");
                    let missing = skipped && s.contains("not found");
                    if !self.status.starts_with("Reattached") {
                        self.status = s;
                    }
                    // Worker owns Props retries (FxReady cancels). UI does not double-retry.
                    if live_ok || skipped {
                        self.params_retry_armed = false;
                        self.params_retry_count = 0;
                    }
                    let _ = missing;
                }
                Event::Error(e) => self.status = e,
                Event::MidiSnapshot(s) => {
                    self.midi_snapshot = s;
                }
                Event::MidiLearnBound { map } => {
                    self.session.midi_maps.retain(|m| {
                        !(m.device_id == map.device_id
                            && m.channel == map.channel
                            && m.controller == map.controller)
                    });
                    self.session.midi_maps.push(map);
                    self.midi_learn = None;
                    self.dirty = true;
                    self.status = "MIDI learn — binding saved".into();
                    self.worker.send(Command::ApplyMidiConfig(self.session.clone()));
                }
                Event::FxReady { .. } => {
                    // Worker owns Props retries; clear UI-side deferral arming.
                    self.params_retry_armed = false;
                    self.params_retry_count = 0;
                }
                Event::SessionApplied {
                    session,
                    message,
                    kind,
                } => {
                    // Typed policy — no message.contains for adopt vs patch.
                    let patch_sinks_only = matches!(
                        kind,
                        SessionAppliedKind::Ensure
                            | SessionAppliedKind::FxRewire
                            | SessionAppliedKind::Route
                            | SessionAppliedKind::Levels
                    ) || message.starts_with("Graph OK")
                        || message.contains("Removed bus")
                        || message.starts_with("Prune");
                    if patch_sinks_only {
                        for t in &session.tracks {
                            if let Some(local) =
                                self.session.tracks.iter_mut().find(|l| l.id == t.id)
                            {
                                if t.sink_name.is_some() {
                                    local.sink_name = t.sink_name.clone();
                                }
                            }
                        }
                        let graph_ok = message.starts_with("Graph OK");
                        self.status = message;
                        if self.viz_live {
                            self.sync_meter_targets();
                        }
                        if graph_ok || self.graph_is_live() {
                            self.graph_loading = false;
                        }
                        continue;
                    }

                    let structural = matches!(
                        kind,
                        SessionAppliedKind::Full | SessionAppliedKind::Clock
                    ) || session.slug != self.session.slug
                        || session.tracks.len() != self.session.tracks.len();
                    if structural {
                        let prev_rate = self.session.performance.sample_rate;
                        let prev_q = self.session.performance.quantum;
                        self.adopt_session(session);
                        let clockish = matches!(kind, SessionAppliedKind::Clock)
                            || prev_rate != self.session.performance.sample_rate
                            || prev_q != self.session.performance.quantum;
                        self.status = message;
                        self.dirty = false;
                        self.graph_loading = false;
                        if clockish {
                            self.force_meter_rebind(true);
                        } else if self.viz_live {
                            self.sync_meter_targets();
                        }
                        self.refresh_performance_from_device();
                    } else {
                        self.status = message;
                        if self.graph_is_live() {
                            self.graph_loading = false;
                        }
                    }
                }
            }
        }

        if let Some(deadline) = self.meter_rebind_deadline {
            if Instant::now() >= deadline {
                if self.viz_live {
                    self.meter_rebind_deadline = None;
                    self.force_meter_rebind(false);
                }
                // else: keep deadline — wake_visualization rebuilds taps on show
            }
        }

        if self.viz_live {
            self.sync_meter_targets();
        }
        self.flush_levels(false);

        if let Some(deadline) = self.hotplug_deadline {
            if Instant::now() >= deadline {
                self.hotplug_deadline = None;
                let dirty: Vec<Uuid> = self.fx_dirty_tracks.drain().collect();
                for track_id in dirty {
                    self.worker.send(Command::RewireTrackFx {
                        session: self.session.clone(),
                        track_id,
                    });
                    self.params_track = Some(track_id);
                    // Props after structural rewire — immediate push.
                    if let Some((bus, inserts)) =
                        crate::audio::insert_map::ladspa_slots_for(&self.session, track_id)
                    {
                        self.worker.send(Command::PushFxControls { bus, inserts });
                    }
                }
                if self.params_track.is_some() {
                    self.status = "Rewiring FX slots (streams untouched)…".into();
                }
            }
        }

        if let Some(deadline) = self.params_deadline {
            if Instant::now() >= deadline {
                // Props may run while rewire is coalescing — ensure is off this thread.
                self.params_deadline = None;
                if let Some(track_id) = self.params_track {
                    if let Some((bus, inserts)) =
                        crate::audio::insert_map::ladspa_slots_for(&self.session, track_id)
                    {
                        self.worker.send(Command::PushFxControls { bus, inserts });
                    } else {
                        self.worker.send(Command::PushFxParams {
                            session: self.session.clone(),
                            track_id,
                        });
                    }
                }
            }
        }
    }
}
