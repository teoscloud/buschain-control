//! Engine runtime — worker-facing handle.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use crate::backend::{
    ensure_clocked_route, link_is_live, sink_exists, AudioBackend, FilterChainRuntime,
    PipewireNativeBackend,
};
use crate::clock::{
    probe_master_hw_from_sinks, probe_sink_running_rate, resolve_profile, set_graph_force_clock,
    wait_hw_running_rate, AudioPreset, DeviceCaps, GraphClock, PerformanceProfile,
};
use crate::contract::{ApplyReport, ClockProps, Intent};
use crate::domain::{
    post_name_for_bus, ChainEnsureMode, ChainSpec, ChainState, InsertSlot,
    LinkSpec, NodeRole, NodeSpec, Props,
};
use crate::fx_gen::{any_gen_live, live_fx_name, live_post_name};
use crate::plan::DesiredState;
use crate::pipeline;

pub struct Engine {
    backend: PipewireNativeBackend,
    desired: DesiredState,
    last_clock: GraphClock,
    fx: FilterChainRuntime,
    /// Last Master HW sink name (for orphan wet probes after restart).
    master_hw: Option<String>,
    /// Debounce preferred-default reassert against WirePlumber.
    last_default_assert: Option<Instant>,
    /// Last applied Pulse monitor mute per bus — avoid pactl spam every idle tick.
    applied_monitor_mute: HashMap<String, bool>,
    /// Idle FX reconcile cadence counter (worker bumps via reconcile_light).
    idle_fx_ticks: u32,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            backend: PipewireNativeBackend::new(),
            desired: DesiredState::default(),
            last_clock: GraphClock::default(),
            fx: FilterChainRuntime::new(),
            master_hw: None,
            last_default_assert: None,
            applied_monitor_mute: HashMap::new(),
            idle_fx_ticks: 0,
        }
    }

    /// Light Master HW switch: update Desired + relink Master→HW only.
    /// Never ArmSession / ForceRespawn — that is what made device switches take minutes.
    pub fn apply_master_hw_relink(&mut self, hw: &str) -> Result<ApplyReport> {
        self.remember_master_hw(hw);
        self.desired.set_master_hw(Some(hw.to_string()));
        // Update Master FX chain dest if present so post→HW targets the new device.
        if let Some(spec) = self.desired.fx_chains.get_mut("buschain_master") {
            spec.dest = hw.to_string();
        }
        if let Some(eg) = self.desired.bus_egress.get_mut("buschain_master") {
            *eg = vec![hw.to_string()];
        }
        let mut report = ApplyReport::default();
        self.reconcile_master_and_default(&mut report)?;
        if report.messages.is_empty() {
            report.push(format!("master hw → {hw}"));
        }
        Ok(report)
    }

    pub fn remember_master_hw(&mut self, hw: &str) {
        if !hw.is_empty() {
            self.master_hw = Some(hw.to_string());
            self.desired.set_master_hw(Some(hw.to_string()));
        }
    }

    pub fn desired(&self) -> &DesiredState {
        &self.desired
    }

    pub fn desired_mut(&mut self) -> &mut DesiredState {
        &mut self.desired
    }

    pub fn clock(&self) -> &GraphClock {
        &self.desired.clock
    }

    pub fn set_profile(&mut self, profile: &PerformanceProfile) {
        let c = profile.graph_clock();
        self.desired.set_clock(c.clone());
        self.last_clock = c;
    }

    /// Force PipeWire graph clock, wait for Master HW, push BusChain props, drop helpers.
    pub fn bind_master_clock_profile(
        &mut self,
        profile: PerformanceProfile,
    ) -> Result<ApplyReport> {
        let mut report = ApplyReport::default();
        let new_clock = profile.graph_clock();
        let hw = if !profile.bound_device.is_empty() {
            profile.bound_device.clone()
        } else {
            self.desired
                .master_hw
                .clone()
                .or_else(|| self.master_hw.clone())
                .unwrap_or_default()
        };
        if !hw.is_empty() {
            self.remember_master_hw(&hw);
        }

        // Balanced / non-Custom: clear force so HW can settle at its preferred rate.
        // Custom: force the selected capable rate (+ quantum unless soft).
        let (force_rate, force_q) = match profile.preset {
            AudioPreset::Custom => (
                profile.sample_rate,
                if profile.soft_quantum {
                    0
                } else {
                    profile.quantum
                },
            ),
            _ => (0, 0),
        };
        match set_graph_force_clock(force_rate, force_q) {
            Ok(()) => {
                crate::clock::invalidate_clock_probe_caches();
                if force_rate > 0 {
                    report.push(format!("force-rate {force_rate}"));
                } else {
                    report.push("force-rate cleared");
                }
            }
            Err(e) => report.push(format!("force-rate: {e}")),
        }

        let want_rate = profile.sample_rate;
        let hw_ok = if hw.is_empty() {
            false
        } else {
            wait_hw_running_rate(&hw, want_rate, Duration::from_secs(3))
        };
        if hw.is_empty() {
            report.push("no Master HW sink — binding BusChain clock only");
        } else if hw_ok {
            report.push(format!("HW running {want_rate} Hz"));
        } else {
            let live = probe_sink_running_rate(&hw)
                .map(|r| r.to_string())
                .unwrap_or_else(|| "?".into());
            report.push(format!(
                "WARN: HW still {live} Hz (wanted {want_rate}) — BusChain bound anyway"
            ));
        }

        let changed = new_clock != self.last_clock;
        self.desired.set_clock(new_clock.clone());

        // Recreate app buses at GraphClock when running rate mismatches (stream migrate).
        let bus_specs: Vec<NodeSpec> = self
            .desired
            .buses
            .values()
            .filter(|s| matches!(s.role, NodeRole::TrackBus | NodeRole::MasterBus))
            .cloned()
            .collect();
        let mut migrated = 0u32;
        for spec in bus_specs {
            let name = spec.name.as_str().to_string();
            let live = probe_sink_running_rate(&name);
            if live != Some(new_clock.sample_rate) {
                match self.backend.migrate_bus_clock(&spec, &new_clock) {
                    Ok(()) => {
                        migrated += 1;
                        report.push(format!(
                            "migrated {name} → {} Hz",
                            new_clock.sample_rate
                        ));
                    }
                    Err(e) => report.push(format!("migrate {name}: {e:#}")),
                }
            } else {
                let props = Props::new()
                    .set("node.latency", new_clock.node_latency_prop())
                    .set("audio.rate", new_clock.sample_rate.to_string())
                    .set("node.force-quantum", new_clock.quantum.to_string())
                    .set(
                        "node.lock-quantum",
                        if new_clock.soft_quantum {
                            "false"
                        } else {
                            "true"
                        },
                    );
                let _ = self.backend.set_props(&name, &props);
            }
        }
        if migrated > 0 {
            report.push(format!("recreated {migrated} app bus(es) at GraphClock"));
        }

        // Drop inbound bridges + post helpers so Hotplug rebuilds at the new clock.
        let _ = self.backend.teardown_rate_bridges();
        self.desired.bridges.clear();
        self.destroy_post_helpers(&mut report);

        if changed {
            report.push(format!(
                "clock bound {} Hz q{}",
                new_clock.sample_rate, new_clock.quantum
            ));
        } else {
            report.push(format!(
                "clock reasserted {} Hz q{}",
                new_clock.sample_rate, new_clock.quantum
            ));
        }
        self.last_clock = new_clock;
        // Mark device_limited false in desired sense — profile already resolved.
        let _ = profile;
        Ok(report)
    }

    fn destroy_post_helpers(&mut self, report: &mut ApplyReport) {
        let buses: Vec<String> = self.desired.buses.keys().cloned().collect();
        let mut n = 0u32;
        for bus in buses {
            let post = post_name_for_bus(&bus);
            if sink_exists(&post) {
                let _ = self.backend.destroy_node(&post);
                n += 1;
            }
        }
        // Also sweep any orphan posts not in desired buses.
        if let Ok(names) = self.backend.list_sink_names() {
            for name in names {
                if name.starts_with("buschain_post_") {
                    let _ = self.backend.destroy_node(&name);
                    n += 1;
                }
            }
        }
        if n > 0 {
            report.push(format!("recreated {n} post helper(s) pending"));
        }
    }

    pub fn probe_master_hw(&mut self, master_output: Option<&str>) -> DeviceCaps {
        let snap = self.backend.snapshot().unwrap_or_default();
        probe_master_hw_from_sinks(&snap.sinks, master_output)
    }

    pub fn apply(&mut self, intent: Intent) -> Result<ApplyReport> {
        let mut report = ApplyReport::default();
        match intent {
            Intent::EnsureBus { spec } => {
                let clock = self.desired.clock.clone();
                self.backend.ensure_node(&spec, &clock)?;
                self.desired.ensure_bus(spec);
                report.push("bus ensured");
            }
            Intent::SetRoute { link } => {
                let clock = self.desired.clock.clone();
                ensure_clocked_route(
                    &mut self.backend,
                    &link.source,
                    &link.sink,
                    &clock,
                    &mut self.desired,
                    link.exclusive,
                )?;
                report.push(format!("route {}→{}", link.source, link.sink));
            }
            Intent::SetLevels {
                sink,
                gain_db,
                muted,
            } => {
                self.backend.set_levels(&sink, gain_db, muted)?;
            }
            Intent::PushProps { node, props } => {
                self.backend.set_props(&node, &props)?;
            }
            Intent::BindMasterClock { profile } => {
                let r = self.bind_master_clock_profile(profile)?;
                for m in r.messages {
                    report.push(m);
                }
            }
            Intent::EnsureFxChain { spec, mode } => {
                if spec.bus.as_str() == "buschain_master" && !spec.dest.is_empty() {
                    self.remember_master_hw(&spec.dest);
                }
                let state = self.ensure_fx_chain(spec, mode)?;
                match &state {
                    ChainState::Wet(w) => {
                        report.push(format!(
                            "FX wet {} → {}",
                            w.fx_sink, w.dest
                        ));
                    }
                    ChainState::Dry => report.push("FX dry"),
                    ChainState::Building => report.push("FX building"),
                    ChainState::Failed(msg) => {
                        return Err(anyhow!("FX chain failed: {msg}"));
                    }
                }
            }
            Intent::PushFxControls { bus, inserts } => {
                self.push_fx_controls(bus.as_str(), inserts)?;
                report.push(format!("FX controls pushed on {}", bus.as_str()));
            }
            Intent::TeardownFxChain { bus } => {
                self.teardown_fx_chain(bus.as_str())?;
                report.push(format!("FX torn down on {}", bus.as_str()));
            }
            Intent::EnsureVirtualInput { bus, description } => {
                let clock = ClockProps::from(&self.desired.clock);
                crate::backend::ensure_virtual_input(bus.as_str(), &description, &clock)?;
                self.desired
                    .set_virtual_input(bus.as_str(), Some(description.clone()));
                if let Some((_, feed)) = crate::backend::virtual_input_names_for_bus(bus.as_str()) {
                    self.desired.ensure_bus(NodeSpec {
                        name: crate::domain::NodeName::new(&feed),
                        description: format!("{description}_VinFeed"),
                        role: NodeRole::VirtualInputFeed,
                        start_muted: false,
                    });
                    let mut dests = self.desired.egress_dests(bus.as_str());
                    if !dests.iter().any(|d| d == &feed) {
                        dests.push(feed.clone());
                        self.desired.set_bus_egress(bus.as_str(), dests.clone());
                    }
                    let wet = self.chain_is_wet(bus.as_str());
                    let _ = self.arm_track_egress(bus.as_str(), wet, &dests);
                }
                report.push(format!("virtual input on {}", bus.as_str()));
            }
            Intent::TeardownVirtualInput { bus } => {
                let _ = crate::backend::teardown_virtual_input(bus.as_str());
                self.desired.set_virtual_input(bus.as_str(), None);
                if let Some((_, feed)) = crate::backend::virtual_input_names_for_bus(bus.as_str()) {
                    self.desired.buses.remove(&feed);
                    let mut dests = self.desired.egress_dests(bus.as_str());
                    dests.retain(|d| d != &feed);
                    self.desired.set_bus_egress(bus.as_str(), dests.clone());
                    let wet = self.chain_is_wet(bus.as_str());
                    let _ = self.arm_track_egress(bus.as_str(), wet, &dests);
                }
                report.push(format!("virtual input off {}", bus.as_str()));
            }
            Intent::Teardown => {
                self.backend.teardown_links();
                let _ = self.backend.teardown_rate_bridges();
                self.fx.stop_all();
                // Tear down in-process hosts for every known bus.
                let buses: Vec<String> = self.desired.fx_chains.keys().cloned().collect();
                for bus in buses {
                    let _ = crate::host::registry::teardown_host(&bus);
                }
                let vins: Vec<String> = self.desired.virtual_inputs.keys().cloned().collect();
                for bus in vins {
                    let _ = crate::backend::teardown_virtual_input(&bus);
                }
                self.desired.clear_routes();
                self.desired.bridges.clear();
                self.desired.fx_chains.clear();
                self.desired.bus_egress.clear();
                self.desired.fx_failed.clear();
                self.desired.virtual_inputs.clear();
                self.desired.speakers_armed = false;
                report.push("engine teardown");
            }
            Intent::Recover => {
                let r = self.reconcile()?;
                for m in r.messages {
                    report.push(m);
                }
            }
            Intent::ArmSession { force_fx } => {
                let r = self.arm_session(force_fx)?;
                for m in r.messages {
                    report.push(m);
                }
            }
            Intent::BindDeviceClock {
                device,
                sample_rate,
                quantum,
                soft_quantum,
                bind_buschain,
            } => {
                if bind_buschain {
                    let profile = PerformanceProfile {
                        preset: AudioPreset::Custom,
                        sample_rate,
                        quantum,
                        soft_quantum,
                        force_suspend_timeout_zero: true,
                        bound_device: device.clone(),
                        device_limited: false,
                    };
                    let r = self.bind_master_clock_profile(profile)?;
                    for m in r.messages {
                        report.push(m);
                    }
                } else {
                    let force_q = if soft_quantum { 0 } else { quantum };
                    match set_graph_force_clock(sample_rate, force_q) {
                        Ok(()) => report.push(format!(
                            "device force-rate {sample_rate} q{force_q} ({device})"
                        )),
                        Err(e) => report.push(format!("force-rate: {e}")),
                    }
                    if wait_hw_running_rate(&device, sample_rate, Duration::from_secs(3)) {
                        report.push(format!("{device} running {sample_rate} Hz"));
                    } else {
                        let live = probe_sink_running_rate(&device)
                            .or_else(|| crate::clock::probe_source_running_rate(&device))
                            .map(|r| r.to_string())
                            .unwrap_or_else(|| "?".into());
                        report.push(format!(
                            "WARN: {device} still {live} Hz (wanted {sample_rate})"
                        ));
                    }
                }
            }
        }
        Ok(report)
    }

    /// Continuous Desired↔live supervisor (buses, FX, Master→HW, default).
    pub fn reconcile(&mut self) -> Result<ApplyReport> {
        // After Tear down / cold desired with speakers still latched off: bring up
        // sealed paths instead of idly disarming Master→HW forever.
        // Prefer warm adopt when PW topology already matches Desired (UI restart).
        if !self.desired.speakers_armed && !self.desired.buses.is_empty() {
            return self.arm_session(true);
        }
        let mut report = self.reconcile_buses_and_levels()?;
        self.reconcile_fx(&mut report);
        self.reconcile_master_and_default(&mut report)?;
        if report.messages.is_empty() {
            report.push("reconcile ok");
        }
        Ok(report)
    }

    /// Idle tick: levels + Master HW + default. FX Idempotent retry every 3rd call
    /// (~6s) so a single spawn miss doesn't leave inserts permanently dry.
    pub fn reconcile_light(&mut self) -> Result<ApplyReport> {
        if !self.desired.speakers_armed && !self.desired.buses.is_empty() {
            return self.arm_session(true);
        }
        let mut report = self.reconcile_buses_and_levels()?;
        // Drop orphan Custom-192k rate bridges after switching back to Balanced.
        self.prune_stale_rate_bridges(&mut report);
        // Every idle tick: kill parallel dry+post paths (chorus/echo).
        self.prune_parallel_fx_routes(&mut report);
        self.reconcile_master_and_default(&mut report)?;
        self.idle_fx_ticks = self.idle_fx_ticks.wrapping_add(1);
        if self.idle_fx_ticks % 3 == 0 {
            self.reconcile_fx(&mut report);
        }
        if report.messages.is_empty() {
            report.push("reconcile light ok");
        }
        Ok(report)
    }

    /// Exclusive routing hygiene — run often; cheap when already clean.
    fn prune_parallel_fx_routes(&mut self, report: &mut ApplyReport) {
        let chains: Vec<ChainSpec> = self.desired.fx_chains.values().cloned().collect();
        for spec in chains {
            let bus = spec.bus.as_str();
            let dest = spec.dest.as_str();
            if dest.is_empty() {
                continue;
            }
            let from = format!("{bus}.monitor");
            // Canonical wet path — prune parallel dry bus→dest when FX host is up.
            let post = live_post_name(bus);
            let post_mon = format!("{post}.monitor");
            let fx = live_fx_name(bus);
            let dests = self.desired.egress_dests(bus);
            let dest_refs: Vec<&str> = dests.iter().map(|s| s.as_str()).collect();
            let allow_egress = if bus == "buschain_master" {
                self.desired.speakers_armed
            } else {
                true
            };

            if self.chain_is_wet(bus) {
                for d in &dests {
                    if link_is_live(&from, d) {
                        let _ = self.backend.unlink_raw(&from, d);
                        report.push(format!("prune dry {bus}→{d} (wet exclusive)"));
                    }
                }
                let mut allow: Vec<&str> = dest_refs.clone();
                allow.push("buschain_hold");
                let _ = self.backend.unlink_from_source_except(&post_mon, &allow);
            } else if !spec.inserts.is_empty() {
                // Feed restore first: spine_ok requires bus→fx. After restart the
                // feed often drops while fx/post remain — pruning post→dest then
                // traps the bus on hold (meters via hold, silence).
                if any_gen_live(bus) && !link_is_live(&from, &fx) {
                    let _ = self.backend.ensure_link_raw(&from, &fx);
                    let _ = self.backend.ensure_link_raw(&from, "buschain_hold");
                    report.push(format!("restore {bus}→fx feed"));
                }
                let spine_now = pipeline::arm::spine_instant_ready(bus);
                if spine_now && allow_egress {
                    let mut missing = false;
                    for d in &dests {
                        if !d.is_empty() && !link_is_live(&post_mon, d) {
                            missing = true;
                            break;
                        }
                    }
                    if missing {
                        wake_sink_for_egress(&dests);
                        match pipeline::arm::arm_track_egress(
                            &mut self.backend,
                            bus,
                            true,
                            &dests,
                        ) {
                            Ok(()) => report.push(format!("re-arm wet egress {bus}")),
                            Err(e) => report.push(format!("re-arm {bus}: {e:#}")),
                        }
                    }
                    for d in &dests {
                        if link_is_live(&from, d) {
                            let _ = self.backend.unlink_raw(&from, d);
                        }
                    }
                } else if !any_gen_live(bus) {
                    // Truly Building — no FX host at all; strip premature post→dest.
                    let mut had_post = false;
                    for d in &dests {
                        if link_is_live(&post_mon, d) {
                            had_post = true;
                            break;
                        }
                    }
                    if had_post || (!dest.is_empty() && link_is_live(&post_mon, dest)) {
                        let _ = self.backend.unlink_from_source_except(&post_mon, &[]);
                        report.push(format!("prune orphan post {post}"));
                    }
                } else {
                    // FX present but spine still settling — keep existing egress;
                    // only drop dry parallel bus→dest.
                    for d in &dests {
                        if link_is_live(&from, &fx) && link_is_live(&from, d) {
                            let _ = self.backend.unlink_raw(&from, d);
                            report.push(format!("prune dry during FX build {bus}→{d}"));
                        }
                    }
                }
            } else if sink_exists(&post) {
                // No inserts Desired but post linger — strip post outs.
                let _ = self.backend.unlink_from_source_except(&post_mon, &[]);
                report.push(format!("prune orphan post {post}"));
            }
        }
    }

    fn prune_stale_rate_bridges(&mut self, report: &mut ApplyReport) {
        let clock_rate = self.desired.clock.sample_rate;
        let Ok(names) = self.backend.list_sink_names() else {
            return;
        };
        let mut removed = 0u32;
        for name in names {
            if !name.starts_with("buschain_rs_") {
                continue;
            }
            let live = crate::clock::probe_sink_running_rate(&name).unwrap_or(0);
            if live != 0 && clock_rate != 0 && live != clock_rate {
                let _ = self.backend.destroy_node(&name);
                removed += 1;
            }
        }
        if removed > 0 {
            let _ = self.backend.teardown_rate_bridges();
            self.desired.bridges.clear();
            report.push(format!("pruned {removed} stale rate-bridge(s)"));
        }
    }

    fn reconcile_buses_and_levels(&mut self) -> Result<ApplyReport> {
        let mut report = ApplyReport::default();
        let clock = self.desired.clock.clone();

        let hold = NodeSpec {
            name: crate::domain::NodeName::new("buschain_hold"),
            description: "BusChainControl_Hold".into(),
            role: NodeRole::Hold,
            start_muted: true,
        };
        let _ = self.backend.ensure_node(&hold, &clock);

        let bus_specs: Vec<NodeSpec> = self.desired.buses.values().cloned().collect();
        for spec in bus_specs {
            if let Err(e) = self.backend.ensure_node(&spec, &clock) {
                report.push(format!("bus {}: {e:#}", spec.name.as_str()));
                continue;
            }
            let name = spec.name.as_str().to_string();
            let mon = format!("{name}.monitor");
            let _ = self.backend.ensure_link_raw(&mon, "buschain_hold");
            if matches!(spec.role, NodeRole::VirtualInputFeed) {
                // Feed must stay open — remap-source masters this monitor.
                let _ = self.backend.open_bus_gain(&name, 0.0);
                continue;
            }
            let level = self
                .desired
                .bus_levels
                .get(&name)
                .copied()
                .unwrap_or_default();
            let _ = self.backend.open_bus_gain(&name, level.gain_db);
            let want_mute = level.mixer_mute;
            let prev = self.applied_monitor_mute.get(&name).copied();
            if prev != Some(want_mute) {
                let _ = self.backend.gate_monitor(&name, want_mute);
                self.applied_monitor_mute.insert(name.clone(), want_mute);
            }
        }
        self.reconcile_virtual_inputs(&mut report);
        Ok(report)
    }

    /// Ensure / prune remap-sources for Desired virtual inputs.
    fn reconcile_virtual_inputs(&mut self, report: &mut ApplyReport) {
        let clock = ClockProps::from(&self.desired.clock);
        let want: Vec<(String, String)> = self
            .desired
            .virtual_inputs
            .iter()
            .map(|(b, d)| (b.clone(), d.clone()))
            .collect();
        for (bus, desc) in &want {
            if let Err(e) = crate::backend::ensure_virtual_input(bus, desc, &clock) {
                report.push(format!("virtual input {bus}: {e:#}"));
            }
        }
        // Drop orphan feeds/sources for track buses no longer flagged.
        let live_feeds: Vec<String> = self
            .desired
            .buses
            .keys()
            .filter(|k| k.starts_with("buschain_vinf_"))
            .cloned()
            .collect();
        for feed in live_feeds {
            let Some(suffix) = feed.strip_prefix("buschain_vinf_") else {
                continue;
            };
            let bus = format!("buschain_track_{suffix}");
            if self.desired.virtual_inputs.contains_key(&bus) {
                continue;
            }
            let _ = crate::backend::teardown_virtual_input(&bus);
            self.desired.buses.remove(&feed);
            report.push(format!("virtual input pruned {bus}"));
        }
    }

    fn reconcile_fx(&mut self, report: &mut ApplyReport) {
        // Tear down live FX for buses that are no longer Desired-wet.
        let keep: std::collections::HashSet<String> =
            self.desired.fx_chains.keys().cloned().collect();
        let buses: Vec<String> = self.desired.buses.keys().cloned().collect();
        for bus in buses {
            if keep.contains(&bus) {
                continue;
            }
            if any_gen_live(&bus) {
                if let Err(e) = self.teardown_fx_chain(&bus) {
                    report.push(format!("FX teardown {bus}: {e:#}"));
                } else {
                    report.push(format!("FX torn down {bus}"));
                }
            }
        }

        let chains: Vec<ChainSpec> = self.desired.fx_chains.values().cloned().collect();
        for spec in chains {
            let bus = spec.bus.as_str().to_string();
            let dest = spec.dest.clone();
            if spec.inserts.is_empty() {
                // Inserts cleared — drop any leftover post→dest (echo source).
                self.prune_orphan_post(&bus, &dest);
                continue;
            }
            let fx = live_fx_name(&bus);
            // Idle must never ForceRespawn — ProbeOnly only.
            match self.ensure_fx_chain(spec, ChainEnsureMode::ProbeOnly) {
                Ok(ChainState::Wet(_)) => {
                    // Exclusive: bus must not also feed dest.
                    let from = format!("{bus}.monitor");
                    if link_is_live(&from, &dest) {
                        let _ = self.backend.unlink_raw(&from, &dest);
                        report.push(format!("FX pruned dry {bus}→{dest}"));
                    }
                    report.push(format!("FX wet {bus}"));
                }
                Ok(ChainState::Failed(e)) => {
                    // ProbeOnly fails when dest is missing even if spine is fine.
                    // Restore bus→fx feed first (common after restart), then re-arm.
                    let from = format!("{bus}.monitor");
                    if any_gen_live(&bus) && !link_is_live(&from, &fx) {
                        let _ = self.backend.ensure_link_raw(&from, &fx);
                        let _ = self.backend.ensure_link_raw(&from, "buschain_hold");
                        report.push(format!("FX {bus}: restored feed"));
                    }
                    if pipeline::arm::spine_instant_ready(&bus) {
                        let dests = self.desired.egress_dests(&bus);
                        let allow = bus != "buschain_master" || self.desired.speakers_armed;
                        if allow && !dests.is_empty() {
                            wake_sink_for_egress(&dests);
                            if pipeline::arm::arm_track_egress(
                                &mut self.backend,
                                &bus,
                                true,
                                &dests,
                            )
                            .is_ok()
                            {
                                report.push(format!("FX {bus}: re-armed egress"));
                            }
                        }
                    } else if !any_gen_live(&bus) {
                        self.prune_orphan_post(&bus, &dest);
                    }
                    if !e.contains("backoff") && !e.contains("not audible") {
                        report.push(format!("FX {bus}: {e}"));
                    }
                }
                Ok(_) => {
                    // Building — only strip post when no FX host exists.
                    if !any_gen_live(&bus) {
                        self.prune_orphan_post(&bus, &dest);
                    }
                }
                Err(e) => {
                    if !any_gen_live(&bus) {
                        self.prune_orphan_post(&bus, &dest);
                    }
                    report.push(format!("FX {bus}: {e:#}"));
                }
            }
        }
    }

    /// Drop post.monitor→* when the spine is actually down. Never strip when the
    /// FX→post path is healthy — missing dest is a re-arm case, not a prune case.
    fn prune_orphan_post(&mut self, bus: &str, dest: &str) {
        if pipeline::arm::spine_instant_ready(bus) {
            return;
        }
        // Post sink for this bus — never strip when spine is healthy.
        let post = live_post_name(bus);
        let post_mon = format!("{post}.monitor");
        if dest.is_empty() {
            let _ = self.backend.unlink_from_source_except(&post_mon, &[]);
            return;
        }
        if link_is_live(&post_mon, dest) || sink_exists(&post) {
            let _ = self.backend.unlink_from_source_except(&post_mon, &[]);
        }
    }

    fn reconcile_master_and_default(&mut self, report: &mut ApplyReport) -> Result<()> {
        if let Some(hw) = self
            .desired
            .master_hw
            .clone()
            .or_else(|| self.master_hw.clone())
        {
            self.remember_master_hw(&hw);
            let master = "buschain_master";
            let from = format!("{master}.monitor");
            let spine_ok = pipeline::arm::spine_instant_ready(master);
            let want_wet = self.desired.fx_chains.contains_key(master);

            // Session barrier: keep speakers silent until cold bring-up arms them.
            if !self.desired.speakers_armed {
                pipeline::arm::disarm_master_hw(&mut self.backend, &hw);
                // Still assert preferred default below.
            } else if want_wet {
                // Wet Master repair must NOT be gated on spine_ok alone.
                // spine_instant_ready requires master.monitor→fx; after restart that
                // feed often drops while fx/post nodes remain. The old branch only
                // restored links when spine_ok was already true → forever on
                // buschain_hold (meters alive via hold/bus, audible silence).
                //
                // CRITICAL: gate on in-process host, not Pulse sink_exists(fx).
                // Duplex PwFxNode is not a null-sink; canonical-only checks stripped
                // the wet path and fail-opened dry master→HW (plugins silently skipped).
                if any_gen_live(master) {
                    let fx = live_fx_name(master);
                    let post = live_post_name(master);
                    let post_mon = format!("{post}.monitor");
                    let _ = self
                        .backend
                        .unlink_from_source_except(&from, &[&fx, "buschain_hold"]);
                    if let Err(e) = self.backend.ensure_link_raw(&from, &fx) {
                        report.push(format!("master→fx feed: {e:#}"));
                    } else if !spine_ok {
                        report.push("master→fx feed restored");
                    }
                    let _ = self.backend.ensure_link_raw(&from, "buschain_hold");
                    if link_is_live(&from, &hw) {
                        let _ = self.backend.unlink_raw(&from, &hw);
                    }
                    // Fresh pw-link listing after feed restore (80ms cache otherwise
                    // says spine still down and skips post→HW in the same tick).
                    crate::backend::invalidate_probe_caches();
                    let feed_ok = link_is_live(&from, &fx);
                    let spine_now = feed_ok && pipeline::arm::spine_instant_ready(master);
                    // Arm egress when feed is up and post exists — don't wait for a
                    // flaky Pulse sink-input probe on a SUSPENDED helper.
                    if feed_ok && sink_exists(&post) && !link_is_live(&post_mon, &hw) {
                        wake_sink(&hw);
                        let _ = self
                            .backend
                            .unlink_from_source_except(&post_mon, &[&hw, "buschain_hold"]);
                        if let Err(e) = self.backend.ensure_link_raw(&post_mon, &hw) {
                            report.push(format!("master post→HW: {e:#}"));
                        } else {
                            report.push(format!("master post→{hw} (re-armed)"));
                            crate::backend::invalidate_probe_caches();
                        }
                    } else if !spine_now && feed_ok {
                        report.push("master spine settling (feed up, waiting post)");
                    }
                } else if self.desired.fx_failed.contains(master) {
                    // FX ensure failed (e.g. legacy plugin labels) — fail-open dry
                    // Master→HW so system audio is not stuck on hold forever.
                    for post in [live_post_name(master), post_name_for_bus(master)] {
                        let post_mon = format!("{post}.monitor");
                        if link_is_live(&post_mon, &hw) || sink_exists(&post) {
                            let _ = self.backend.unlink_from_source_except(&post_mon, &[]);
                        }
                    }
                    wake_sink(&hw);
                    let _ = self
                        .backend
                        .unlink_from_source_except(&from, &[&hw, "buschain_hold"]);
                    if let Err(e) = self.backend.ensure_link_raw(&from, &hw) {
                        report.push(format!("master→HW (FX failed, dry): {e:#}"));
                    } else {
                        report.push(format!("master→{hw} (FX failed — dry fail-open)"));
                    }
                    let _ = self.backend.ensure_link_raw(&from, "buschain_hold");
                } else {
                    // FX node missing mid-build — hold only; never arm dry Master→HW.
                    let _ = self
                        .backend
                        .unlink_from_source_except(&from, &["buschain_hold"]);
                    let _ = self.backend.ensure_link_raw(&from, "buschain_hold");
                    if link_is_live(&from, &hw) {
                        let _ = self.backend.unlink_raw(&from, &hw);
                    }
                }
            } else {
                // Dry Master: never leave orphan post→HW (parallel with master→HW =
                // delayed double = chorus/echo). Prune post outs every idle pass.
                for post in [live_post_name(master), post_name_for_bus(master)] {
                    let post_mon = format!("{post}.monitor");
                    if link_is_live(&post_mon, &hw) || sink_exists(&post) {
                        let _ = self.backend.unlink_from_source_except(&post_mon, &[]);
                    }
                }
                if link_is_live(&from, &hw) {
                    // Healthy dry Master→HW — leave master links alone (meter stability).
                } else {
                    wake_sink(&hw);
                    let _ = self
                        .backend
                        .unlink_from_source_except(&from, &[&hw, "buschain_hold"]);
                    if let Err(e) = self.backend.ensure_link_raw(&from, &hw) {
                        report.push(format!("master→HW: {e:#}"));
                    } else {
                        report.push(format!("master→{hw} (re-armed)"));
                    }
                }
            }
        }

        if let Some(pref) = self.desired.preferred_default.clone() {
            let exists = self
                .backend
                .list_sink_names()
                .map(|n| n.iter().any(|s| s == &pref))
                .unwrap_or(false);
            if exists {
                let live = self.backend.default_sink_name();
                if live.as_deref() != Some(pref.as_str()) {
                    let due = self
                        .last_default_assert
                        .map(|t| t.elapsed() >= Duration::from_millis(500))
                        .unwrap_or(true);
                    if due {
                        self.last_default_assert = Some(Instant::now());
                        match self.backend.set_default_sink(&pref) {
                            Ok(true) => report.push(format!("default→{pref}")),
                            Ok(false) => {
                                report.push(format!("default did not stick — retrying ({pref})"))
                            }
                            Err(e) => report.push(format!("default: {e:#}")),
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn ensure_fx_chain(
        &mut self,
        spec: ChainSpec,
        mode: ChainEnsureMode,
    ) -> Result<ChainState> {
        if spec.bus.as_str() == "buschain_master" && !spec.dest.is_empty() {
            self.remember_master_hw(&spec.dest);
        }
        let bus = spec.bus.as_str().to_string();
        // Master→HW stays behind the session barrier until speakers_armed.
        let arm_egress = if bus == "buschain_master" {
            self.desired.speakers_armed
        } else {
            true
        };
        let clock = self.desired.clock.clone();
        let state = pipeline::insert::ensure_fx_chain(
            &mut self.fx,
            &mut self.backend,
            &clock,
            &spec,
            mode,
            arm_egress,
        )?;
        if matches!(state, ChainState::Failed(_)) {
            self.desired.fx_failed.insert(bus.clone());
            return Ok(state);
        }
        self.desired.fx_failed.remove(&bus);
        if spec.inserts.is_empty() {
            self.desired.remove_fx_chain(spec.bus.as_str());
        } else if state.is_wet() {
            self.desired.ensure_fx_chain(spec);
            // Multi-dest: arm every configured hop after primary wet land.
            if arm_egress {
                let dests = self.desired.egress_dests(&bus);
                if dests.len() > 1 {
                    let _ = pipeline::arm::arm_track_egress(
                        &mut self.backend,
                        &bus,
                        true,
                        &dests,
                    );
                }
            }
        }
        Ok(state)
    }

    /// True when live PipeWire already matches Desired (buses, FX spines, Master→HW).
    /// Fail-closed — any mismatch means cold ArmSession.
    pub fn session_graph_healthy(&self) -> bool {
        if self.desired.buses.is_empty() {
            return false;
        }
        let hw = self
            .desired
            .master_hw
            .clone()
            .or_else(|| self.master_hw.clone())
            .unwrap_or_default();

        for bus in self.desired.buses.keys() {
            // Helpers are not egress-armed track buses.
            if bus.starts_with("buschain_vinf_")
                || bus.starts_with("buschain_vin_")
                || bus == "buschain_hold"
            {
                continue;
            }
            if !sink_exists(bus) {
                return false;
            }
            let dests = self.desired.egress_dests(bus);
            if let Some(spec) = self.desired.fx_chains.get(bus) {
                if spec.inserts.is_empty() {
                    return false;
                }
                // Warm adopt: matching live `.sig` + helper/post sinks present.
                // Do NOT require spine_instant_ready — at UI restart that probe
                // false-negatives under pactl load and forced ForceRespawn of
                // every track (~10s each) while audio was already left running.
                let want = spec.signature();
                let post = live_post_name(bus);
                // In-process host only — duplex FX is not a Pulse sink.
                let fp_ok =
                    crate::host::registry::host_fingerprint(bus).as_deref() == Some(want.as_str());
                let host_ok = crate::host::registry::host_running(bus);
                if !fp_ok || !host_ok || !sink_exists(&post) {
                    return false;
                }
                // Missing post→dest must NOT force ForceRespawn — adopt re-arms
                // egress link-only (meters via hold; audio resumes quickly).
                if bus == "buschain_master" {
                    if hw.is_empty() {
                        return false;
                    }
                } else if dests.iter().all(|d| d.is_empty()) {
                    return false;
                }
            } else {
                // Dry bus.
                if !pipeline::arm::track_path_ready(bus, false, &dests) {
                    return false;
                }
                let from = format!("{bus}.monitor");
                if bus == "buschain_master" {
                    if hw.is_empty() || !link_is_live(&from, &hw) {
                        return false;
                    }
                } else {
                    let mut linked = false;
                    for d in &dests {
                        if !d.is_empty() && link_is_live(&from, d) {
                            linked = true;
                            break;
                        }
                    }
                    if !linked {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Warm restart: latch speakers and Idempotent-reconcile without disarm/ForceRespawn.
    pub fn adopt_live_session(&mut self) -> Result<ApplyReport> {
        let mut report = ApplyReport::default();
        report.push("warm adopt — graph already live");
        self.desired.speakers_armed = true;
        self.desired.fx_failed.clear();
        let mut bus_report = self.reconcile_buses_and_levels()?;
        report.messages.append(&mut bus_report.messages);
        self.reconcile_fx(&mut report);
        self.reconcile_master_and_default(&mut report)?;
        report.push("speakers armed (adopted)");
        Ok(report)
    }

    /// Cold bring-up / Full Apply: sealed disarm → spines → dwell → track arm → barrier → HW.
    /// When the live graph already matches Desired, warm-adopts instead (UI restart).
    pub fn arm_session(&mut self, force_fx: bool) -> Result<ApplyReport> {
        if self.session_graph_healthy() {
            return self.adopt_live_session();
        }

        let mut report = ApplyReport::default();
        self.desired.speakers_armed = false;
        self.desired.fx_failed.clear();

        let buses: Vec<String> = self.desired.buses.keys().cloned().collect();
        for bus in &buses {
            let keep_fx = self.desired.fx_chains.contains_key(bus);
            pipeline::arm::disarm_track_egress(&mut self.backend, bus, keep_fx);
        }
        // Soft-arm Master: do not silence speakers if Master→HW is already live.
        // Cold disarm-first caused multi-second mute on UI restart (fail-closed).
        if let Some(hw) = self
            .desired
            .master_hw
            .clone()
            .or_else(|| self.master_hw.clone())
        {
            let master_live = link_is_live("buschain_master.monitor", &hw)
                || link_is_live("buschain_post_master.monitor", &hw);
            if master_live {
                report.push(format!("soft-arm speakers (keep live →{hw})"));
            } else {
                pipeline::arm::disarm_master_hw(&mut self.backend, &hw);
                report.push(format!("disarmed speakers ({hw})"));
            }
        }

        let mut bus_report = self.reconcile_buses_and_levels()?;
        report.messages.append(&mut bus_report.messages);

        let mode = if force_fx {
            ChainEnsureMode::ForceRespawn
        } else {
            ChainEnsureMode::Idempotent
        };
        let chains: Vec<ChainSpec> = self.desired.fx_chains.values().cloned().collect();
        // Parallel host bring-up — filter threads + LADSPA load overlap.
        // Then link/arm per bus (CLI link plane still serial).
        let force_host = matches!(mode, ChainEnsureMode::ForceRespawn);
        let need: Vec<ChainSpec> = chains
            .iter()
            .filter(|s| !s.inserts.is_empty())
            .filter(|s| {
                force_host
                    || !crate::host::registry::host_running(s.bus.as_str())
                    || crate::host::registry::host_fingerprint(s.bus.as_str()).as_deref()
                        != Some(s.signature().as_str())
            })
            .cloned()
            .collect();
        if !need.is_empty() {
            let clock = self.desired.clock.clone();
            let t_hosts = std::time::Instant::now();
            for (bus, r) in crate::host::registry::ensure_hosts_parallel(&need, &clock, true) {
                if let Err(e) = r {
                    report.push(format!("FX host {bus}: {e:#}"));
                }
            }
            report.push(format!(
                "FX hosts parallel {} in {}ms",
                need.len(),
                t_hosts.elapsed().as_millis()
            ));
        }
        // Link/arm only — racks already published by the parallel host pass.
        let _ = mode;
        for spec in chains {
            let bus = spec.bus.as_str().to_string();
            match self.ensure_fx_chain(spec, ChainEnsureMode::Idempotent) {
                Ok(ChainState::Wet(_)) => report.push(format!("FX wet {bus}")),
                Ok(ChainState::Failed(e)) => {
                    if !e.contains("backoff") {
                        report.push(format!("FX {bus} failed (dry): {e}"));
                    }
                }
                Ok(ChainState::Dry) => report.push(format!("FX dry {bus}")),
                Ok(ChainState::Building) => report.push(format!("FX building {bus}")),
                Err(e) => report.push(format!("FX {bus}: {e:#}")),
            }
        }

        // Dry (no-insert) tracks: short dwell then arm all egress hops.
        for bus in &buses {
            if bus == "buschain_master" {
                continue;
            }
            if self.desired.fx_chains.contains_key(bus) {
                continue;
            }
            let dests = self.desired.egress_dests(bus);
            if !pipeline::arm::wait_dry_spine_stable(bus, pipeline::arm::DRY_DWELL) {
                report.push(format!("dry {bus}: bus not ready"));
                continue;
            }
            if let Err(e) =
                pipeline::arm::arm_track_egress(&mut self.backend, bus, false, &dests)
            {
                report.push(format!("dry arm {bus}: {e:#}"));
            } else {
                report.push(format!("dry armed {bus}"));
            }
        }

        // Wet tracks may have only armed primary dest — refresh multi-dest.
        for bus in &buses {
            if bus == "buschain_master" {
                continue;
            }
            if !self.desired.fx_chains.contains_key(bus) {
                continue;
            }
            if self.desired.fx_failed.contains(bus) {
                let dests = self.desired.egress_dests(bus);
                let _ = pipeline::arm::arm_track_egress(&mut self.backend, bus, false, &dests);
                continue;
            }
            if pipeline::arm::spine_instant_ready(bus) || self.chain_is_wet(bus) {
                let dests = self.desired.egress_dests(bus);
                let _ = pipeline::arm::arm_track_egress(&mut self.backend, bus, true, &dests);
            }
        }

        // Session barrier — host atomics + short wait (was 80×50ms CLI spine polls).
        let mut cleared = false;
        for _ in 0..20 {
            if self.session_barrier_clear() {
                cleared = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !cleared {
            report.push("session barrier timeout — arming speakers fail-open");
        } else {
            report.push("session barrier clear");
        }

        if let Some(hw) = self
            .desired
            .master_hw
            .clone()
            .or_else(|| self.master_hw.clone())
        {
            let wet = !self.desired.fx_failed.contains("buschain_master")
                && self.desired.fx_chains.contains_key("buschain_master")
                && (crate::host::registry::host_running("buschain_master")
                    || pipeline::arm::spine_instant_ready("buschain_master")
                    || self.chain_is_wet("buschain_master"));
            if let Err(e) = pipeline::arm::arm_master_hw(&mut self.backend, &hw, wet) {
                report.push(format!("arm Master→HW: {e:#}"));
            } else {
                report.push(format!(
                    "armed Master→{hw} ({})",
                    if wet { "wet" } else { "dry" }
                ));
            }
        }
        self.desired.speakers_armed = true;
        report.push("speakers armed");
        // Second pass: restore feed + post→HW if cold arm left Master on hold
        // (common when sink_has_input flaked during the barrier window).
        crate::backend::invalidate_probe_caches();
        self.reconcile_master_and_default(&mut report)?;
        Ok(report)
    }

    /// Master spine ready + every unmuted non-master insert track is Wet or Failed.
    pub fn session_barrier_clear(&mut self) -> bool {
        // Master spine
        let master_ok = if self.desired.fx_chains.contains_key("buschain_master") {
            if self.desired.fx_failed.contains("buschain_master") {
                sink_exists("buschain_master")
            } else {
                pipeline::arm::spine_instant_ready("buschain_master")
                    || self.chain_is_wet("buschain_master")
            }
        } else {
            pipeline::arm::dry_spine_instant_ready("buschain_master")
        };
        if !master_ok {
            return false;
        }

        let chains: Vec<(String, bool)> = self
            .desired
            .fx_chains
            .keys()
            .filter(|b| b.as_str() != "buschain_master")
            .map(|b| {
                let muted = self
                    .desired
                    .bus_levels
                    .get(b)
                    .map(|l| l.mixer_mute)
                    .unwrap_or(false);
                (b.clone(), muted)
            })
            .collect();

        for (bus, muted) in chains {
            if muted {
                continue;
            }
            if self.desired.fx_failed.contains(&bus) {
                continue; // Failed + dry restored = resolved
            }
            if !(self.chain_is_wet(&bus) || pipeline::arm::spine_instant_ready(&bus)) {
                return false;
            }
        }
        true
    }

    pub fn push_fx_controls(&mut self, bus: &str, inserts: Vec<InsertSlot>) -> Result<()> {
        pipeline::insert::push_fx_controls(&mut self.fx, bus, &inserts)?;
        if let Some(spec) = self.desired.fx_chains.get_mut(bus) {
            spec.inserts = inserts;
        }
        Ok(())
    }

    /// Idle tick that skips Idempotent FX respawn (use when knobs just ran).
    pub fn reconcile_light_no_fx(&mut self) -> Result<ApplyReport> {
        if !self.desired.speakers_armed && !self.desired.buses.is_empty() {
            return self.arm_session(true);
        }
        let mut report = self.reconcile_buses_and_levels()?;
        self.prune_stale_rate_bridges(&mut report);
        self.prune_parallel_fx_routes(&mut report);
        self.reconcile_master_and_default(&mut report)?;
        if report.messages.is_empty() {
            report.push("reconcile light (no fx) ok");
        }
        Ok(report)
    }

    /// Route / Hotplug: Desired sync already applied — relink egress + Master HW only.
    /// Never ForceRespawn / reconcile_fx.
    pub fn relink_routes(&mut self) -> Result<ApplyReport> {
        if !self.desired.speakers_armed && !self.desired.buses.is_empty() {
            return self.arm_session(true);
        }
        let mut report = ApplyReport::default();
        // Re-arm every bus egress from Desired (listen / outs / Master).
        let buses: Vec<String> = self.desired.buses.keys().cloned().collect();
        for bus in buses {
            if bus.starts_with("buschain_vinf_") || bus == "buschain_hold" {
                continue;
            }
            let dests = self
                .desired
                .bus_egress
                .get(&bus)
                .cloned()
                .unwrap_or_default();
            let keep_fx = self
                .desired
                .fx_chains
                .get(&bus)
                .is_some_and(|c| !c.inserts.is_empty());
            if let Err(e) = self.arm_track_egress(&bus, keep_fx, &dests) {
                report.push(format!("relink {bus}: {e:#}"));
            }
        }
        self.reconcile_virtual_inputs(&mut report);
        self.prune_parallel_fx_routes(&mut report);
        self.reconcile_master_and_default(&mut report)?;
        if report.messages.is_empty() {
            report.push("relink routes ok");
        }
        Ok(report)
    }

    pub fn teardown_fx_chain(&mut self, bus: &str) -> Result<()> {
        pipeline::insert::teardown_fx_chain(&mut self.fx, &mut self.backend, bus)?;
        self.desired.remove_fx_chain(bus);
        Ok(())
    }

    /// Probe wetness from live PW topology (works even if DesiredState was cleared on restart).
    pub fn chain_state(&mut self, bus: &str) -> ChainState {
        let (inserts_len, dest) = match self.desired.fx_chains.get(bus) {
            Some(spec) if !spec.inserts.is_empty() => {
                (spec.inserts.len(), spec.dest.clone())
            }
            _ => {
                // Orphan / restart: treat existing FX host as a candidate wet path.
                if !any_gen_live(bus) {
                    return ChainState::Dry;
                }
                let dest = if bus == "buschain_master" {
                    self.master_hw
                        .clone()
                        .unwrap_or_else(|| "buschain_master".into())
                } else {
                    "buschain_master".into()
                };
                (1, dest)
            }
        };
        // Master spine can be Wet before session barrier opens post→HW.
        let require_dest = if bus == "buschain_master" {
            self.desired.speakers_armed
        } else {
            true
        };
        pipeline::insert::probe_chain_state(
            &mut self.fx,
            bus,
            inserts_len,
            &dest,
            require_dest,
        )
    }

    /// True when bus.monitor → fx → post is carrying audio (meter / route gate).
    /// Uses host atomics + spine probe — never shells out per idle/barrier tick.
    pub fn chain_is_wet(&mut self, bus: &str) -> bool {
        if self.desired.fx_failed.contains(bus) {
            return false;
        }
        if let Some(spec) = self.desired.fx_chains.get(bus) {
            if spec.inserts.is_empty() {
                return false;
            }
            if !crate::host::registry::host_running(bus) {
                return false;
            }
            // Master post→HW stays gated until speakers_armed; host up is enough here.
            if bus == "buschain_master" && !self.desired.speakers_armed {
                return true;
            }
            return pipeline::arm::spine_instant_ready(bus);
        }
        any_gen_live(bus) && pipeline::arm::spine_instant_ready(bus)
    }

    pub fn stop_all_fx(&mut self) {
        self.fx.stop_all();
        self.desired.fx_chains.clear();
    }

    /// Convenience: ensure null-sink with current clock (used by graph shim).
    pub fn ensure_bus(
        &mut self,
        name: &str,
        description: &str,
        role: NodeRole,
        start_muted: bool,
    ) -> Result<()> {
        self.apply(Intent::EnsureBus {
            spec: NodeSpec {
                name: crate::domain::NodeName::new(name),
                description: description.into(),
                role,
                start_muted,
            },
        })?;
        Ok(())
    }

    pub fn ensure_route(&mut self, source: &str, sink: &str, exclusive: bool) -> Result<()> {
        self.apply(Intent::SetRoute {
            link: LinkSpec {
                source: source.into(),
                sink: sink.into(),
                exclusive,
            },
        })?;
        Ok(())
    }

    pub fn disarm_track_egress(&mut self, bus: &str, keep_fx_feed: bool) {
        pipeline::arm::disarm_track_egress(&mut self.backend, bus, keep_fx_feed);
    }

    pub fn arm_track_egress(&mut self, bus: &str, wet: bool, dests: &[String]) -> Result<()> {
        pipeline::arm::arm_track_egress(&mut self.backend, bus, wet, dests)
    }

    pub fn ensure_link_raw(&mut self, source: &str, sink: &str) -> Result<()> {
        self.backend.ensure_link_raw(source, sink)?;
        Ok(())
    }

    pub fn unlink_raw(&mut self, source: &str, sink: &str) -> Result<()> {
        self.backend.unlink_raw(source, sink)
    }

    pub fn unlink_from_source_except(&mut self, source: &str, allow: &[&str]) -> u32 {
        self.backend.unlink_from_source_except(source, allow)
    }

    pub fn teardown_links(&mut self) {
        self.backend.teardown_links();
        let _ = self.backend.teardown_rate_bridges();
    }

    pub fn destroy_rate_bridges_only(&mut self) {
        let _ = self.backend.teardown_rate_bridges();
        self.desired.bridges.clear();
    }

    pub fn link_is_live(source: &str, sink: &str) -> bool {
        crate::backend::link_is_live(source, sink)
    }

    pub fn snapshot(&mut self) -> Result<crate::domain::GraphSnapshot> {
        self.backend.snapshot()
    }

    pub fn set_default_sink(&mut self, name: &str) -> Result<bool> {
        self.backend.set_default_sink(name)
    }

    pub fn resolve_profile(
        preset: AudioPreset,
        caps: &DeviceCaps,
        custom_rate: Option<u32>,
        custom_quantum: Option<u32>,
        soft_quantum: bool,
    ) -> PerformanceProfile {
        resolve_profile(preset, caps, custom_rate, custom_quantum, soft_quantum)
    }

    pub fn filter_chain_clock_fragment(&self) -> String {
        crate::backend::filter_chain_clock_props(&self.desired.clock)
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// Wake a PipeWire/Pulse sink that may have auto-suspended (USB idle, etc.).
fn wake_sink(name: &str) {
    if name.is_empty() {
        return;
    }
    let _ = std::process::Command::new("pactl")
        .args(["suspend-sink", name, "0"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn wake_sink_for_egress(dests: &[String]) {
    for d in dests {
        // Only wake real HW / non-buschain sinks; buschain_* buses stay hot via keepalive.
        if d.is_empty() || d.starts_with("buschain_") {
            continue;
        }
        wake_sink(d);
    }
}
