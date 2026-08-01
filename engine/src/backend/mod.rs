//! AudioBackend trait + PipeWire implementations (native default, CLI fallback).

mod cli;
mod fx_chain;
mod link;
mod native;
mod null_sink;
mod playback;
mod virtual_input;
pub mod pulse_compat;

pub use playback::enforce_desired_playback;

pub use virtual_input::{
    ensure_virtual_input, is_virtual_input_feed, is_virtual_input_source, names_for_bus as virtual_input_names_for_bus,
    push_virtual_input_description, teardown_virtual_input,
};

pub use cli::{
    invalidate_probe_caches, pactl_short_sinks, pw_link_inputs, pw_link_outputs,
};
pub use link::{
    ensure_link_force, pulse_loopback_owned, unlink_capture_into_sink_except,
    unlink_capture_into_sink_except_with_bridges, unload_legacy_from_source_except,
    unload_legacy_loopback, unload_legacy_loopback_force,
    unload_legacy_loopbacks_into_sink_except, unload_orphan_hw_to_rs_loopbacks,
    wait_sink_playback_ports,
};

pub use fx_chain::{
    cache_node_id, find_node_id_by_name, ladspa_search_path, sink_exists, sink_has_input,
    FilterChainRuntime,
};

pub use native::{list_midi_nodes, native_ready, PipewireNativeBackend};

use anyhow::{anyhow, Context, Result};

use crate::clock::{probe_endpoint_caps, probe_sink_running_rate, EndpointCaps, GraphClock};
use crate::contract::ClockProps;
use crate::domain::{
    DeviceNode, GraphSnapshot, LinkId, LinkSpec, NodeId, NodeName, NodeRole, NodeSpec, Props,
};
use crate::plan::DesiredState;

/// Gate outbound Pulse `{bus}.monitor` source without muting the app-facing sink.
///
/// Never SPA `set_levels` on `*.monitor` — native bind strips `.monitor` and would
/// mute/cork the sink node (broke mixer mute for tracks with In/Apps).
pub fn gate_bus_monitor(bus: &str, gated: bool) -> Result<()> {
    let mon = format!("{bus}.monitor");
    if gated {
        let _ = run_ok("pactl", &["set-source-mute", &mon, "1"]);
        let _ = run_ok("pactl", &["set-source-volume", &mon, "0%"]);
    } else {
        let _ = run_ok("pactl", &["set-source-mute", &mon, "0"]);
        let _ = run_ok("pactl", &["set-source-volume", &mon, "100%"]);
    }
    Ok(())
}

fn allow_pulse_capture_fallback() -> bool {
    matches!(
        std::env::var("BUSCHAIN_ALLOW_PULSE_CAPTURE").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

/// Prefer native registry links; Pulse/CLI only when registry down or escape hatch.
pub fn ensure_link(source: &str, sink: &str) -> Result<()> {
    if native::native_ready() {
        match native::native_ensure_link(source, sink) {
            Ok(()) => {
                if native::native_link_is_live(source, sink) {
                    return Ok(());
                }
                // Native RPC succeeded but registry doesn't see the hop yet — brief wait.
                for _ in 0..4 {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    if native::native_link_is_live(source, sink) {
                        return Ok(());
                    }
                }
                if !allow_pulse_capture_fallback() {
                    return Err(anyhow!(
                        "native ensure ok but hop not live {source}→{sink}"
                    ));
                }
            }
            Err(e) => {
                let dst = sink.strip_suffix(".monitor").unwrap_or(sink);
                let src = source.strip_suffix(".monitor").unwrap_or(source);
                if dst.starts_with("buschain_fx_") || src.starts_with("buschain_fx_") {
                    return Err(e);
                }
                if !allow_pulse_capture_fallback() {
                    return Err(e);
                }
            }
        }
    }
    link::ensure_link(source, sink)
}

/// Force-recreate a hop (capture Add after mute/×).
pub fn ensure_link_force_pair(source: &str, sink: &str) -> Result<()> {
    let _ = unlink(source, sink);
    if native::native_ready() {
        match native::native_ensure_link(source, sink) {
            Ok(()) if native::native_link_is_live(source, sink) => return Ok(()),
            Ok(()) => {
                for _ in 0..4 {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    if native::native_link_is_live(source, sink) {
                        return Ok(());
                    }
                }
                if !allow_pulse_capture_fallback() {
                    return Err(anyhow!(
                        "native force-ensure ok but hop not live {source}→{sink}"
                    ));
                }
            }
            Err(e) => {
                let dst = sink.strip_suffix(".monitor").unwrap_or(sink);
                let src = source.strip_suffix(".monitor").unwrap_or(source);
                if dst.starts_with("buschain_fx_") || src.starts_with("buschain_fx_") {
                    return Err(e);
                }
                if !allow_pulse_capture_fallback() {
                    return Err(e);
                }
            }
        }
    }
    link::ensure_link_force(source, sink)
}

/// Capture hop verified-live: direct source→bus OR Mic→rs AND rs.monitor→bus.
pub fn capture_hop_verified(source: &str, bus: &str, desired: &DesiredState) -> bool {
    if link_is_live(source, bus) {
        return true;
    }
    for (s, d) in &desired.routes {
        if s != source {
            continue;
        }
        if !(d.starts_with("buschain_rs_") || d.starts_with("shadow_rs_")) {
            continue;
        }
        let mon = format!("{d}.monitor");
        if link_is_live(source, d) && link_is_live(&mon, bus) {
            return true;
        }
    }
    false
}

pub fn unlink(source: &str, sink: &str) -> Result<()> {
    if native::native_ready() {
        let _ = native::native_unlink(source, sink);
    }
    link::unlink(source, sink)
}

pub fn link_is_live(source: &str, sink: &str) -> bool {
    // Native registry ready ⇒ false is final (no CLI fallthrough on expected misses).
    if native::native_ready() {
        return native::native_link_is_live(source, sink);
    }
    link::link_is_live(source, sink)
}

pub use link::teardown_buschain_links;

pub use null_sink::push_description;

pub trait AudioBackend {
    fn snapshot(&mut self) -> Result<GraphSnapshot>;
    fn ensure_node(&mut self, spec: &NodeSpec, clock: &GraphClock) -> Result<NodeId>;
    fn destroy_node(&mut self, name: &str) -> Result<()>;
    fn ensure_link_raw(&mut self, source: &str, sink: &str) -> Result<LinkId>;
    fn unlink_raw(&mut self, source: &str, sink: &str) -> Result<()>;
    fn unlink_from_source_except(&mut self, source: &str, allow_sinks: &[&str]) -> u32;
    fn set_props(&mut self, node: &str, props: &Props) -> Result<()>;
    fn probe_endpoint(&mut self, name: &str) -> Result<EndpointCaps>;
    fn set_levels(&mut self, sink: &str, gain_db: f32, muted: bool) -> Result<()>;
    /// Open app-facing bus at gain — never leave sink muted (corking).
    fn open_bus_gain(&mut self, sink: &str, gain_db: f32) -> Result<()>;
    /// Gate outbound monitor only (mixer mute).
    fn gate_monitor(&mut self, bus: &str, gated: bool) -> Result<()>;
    fn list_sink_names(&mut self) -> Result<Vec<String>>;
    fn default_sink_name(&mut self) -> Option<String>;
    fn set_default_sink(&mut self, name: &str) -> Result<bool>;
    fn teardown_links(&mut self);
    fn teardown_rate_bridges(&mut self) -> Result<()>;
    /// Recreate an app bus at GraphClock when running rate mismatches (migrate streams).
    fn migrate_bus_clock(&mut self, spec: &NodeSpec, clock: &GraphClock) -> Result<()>;
}

pub struct PipewireCliBackend;

impl Default for PipewireCliBackend {
    fn default() -> Self {
        Self
    }
}

impl PipewireCliBackend {
    pub fn new() -> Self {
        Self
    }
}

impl AudioBackend for PipewireCliBackend {
    fn snapshot(&mut self) -> Result<GraphSnapshot> {
        Ok(GraphSnapshot {
            sinks: list_devices("sinks")?,
            sources: list_devices("sources")?,
        })
    }

    fn ensure_node(&mut self, spec: &NodeSpec, clock: &GraphClock) -> Result<NodeId> {
        null_sink::ensure_null_sink(spec, &ClockProps::from(clock))?;
        Ok(NodeId(spec.name.0.clone()))
    }

    fn destroy_node(&mut self, name: &str) -> Result<()> {
        null_sink::unload_named_null_sink(name)
    }

    fn ensure_link_raw(&mut self, source: &str, sink: &str) -> Result<LinkId> {
        link::ensure_link(source, sink)?;
        Ok(LinkId(format!("{source}->{sink}")))
    }

    fn unlink_raw(&mut self, source: &str, sink: &str) -> Result<()> {
        link::unlink(source, sink)
    }

    fn unlink_from_source_except(&mut self, source: &str, allow_sinks: &[&str]) -> u32 {
        link::unlink_from_source_except(source, allow_sinks)
    }

    fn set_props(&mut self, node: &str, props: &Props) -> Result<()> {
        set_node_props_cli(node, props)
    }

    fn probe_endpoint(&mut self, name: &str) -> Result<EndpointCaps> {
        Ok(probe_endpoint_caps(name))
    }

    fn set_levels(&mut self, sink: &str, gain_db: f32, muted: bool) -> Result<()> {
        let linear = if muted {
            0.0
        } else {
            10f32.powf(gain_db / 20.0)
        };
        let pct = (linear * 100.0).clamp(0.0, 150.0).round() as u32;
        run_ok("pactl", &["set-sink-volume", sink, &format!("{pct}%")])?;
        run_ok("pactl", &["set-sink-mute", sink, if muted { "1" } else { "0" }])?;
        Ok(())
    }

    fn open_bus_gain(&mut self, sink: &str, gain_db: f32) -> Result<()> {
        let linear = 10f32.powf(gain_db / 20.0);
        let pct = (linear * 100.0).clamp(0.0, 150.0).round() as u32;
        run_ok("pactl", &["set-sink-volume", sink, &format!("{pct}%")])?;
        run_ok("pactl", &["set-sink-mute", sink, "0"])?;
        Ok(())
    }

    fn gate_monitor(&mut self, bus: &str, gated: bool) -> Result<()> {
        gate_bus_monitor(bus, gated)
    }

    fn list_sink_names(&mut self) -> Result<Vec<String>> {
        Ok(list_devices("sinks")?
            .into_iter()
            .map(|d| d.name)
            .collect())
    }

    fn default_sink_name(&mut self) -> Option<String> {
        let out = std::process::Command::new("pactl")
            .args(["info"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("Default Sink:") {
                let name = rest.trim();
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
        None
    }

    fn set_default_sink(&mut self, name: &str) -> Result<bool> {
        if self.default_sink_name().as_deref() == Some(name) {
            return Ok(true);
        }
        match run_ok("pactl", &["set-default-sink", name]) {
            Ok(()) => {
                std::thread::sleep(std::time::Duration::from_millis(40));
                if self.default_sink_name().as_deref() == Some(name) {
                    return Ok(true);
                }
            }
            Err(_) => {}
        }
        // wpctl fallback by sink index
        if let Ok(sinks) = list_devices("sinks") {
            if let Some((i, _)) = sinks.iter().enumerate().find(|(_, s)| s.name == name) {
                let _ = std::process::Command::new("wpctl")
                    .args(["set-default", &i.to_string()])
                    .status();
            }
        }
        // Retry pactl once more with short list index
        let short = std::process::Command::new("pactl")
            .args(["list", "short", "sinks"])
            .output();
        if let Ok(out) = short {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                let mut parts = line.split('\t');
                let Some(idx) = parts.next() else { continue };
                let Some(n) = parts.next() else { continue };
                if n == name {
                    let _ = std::process::Command::new("wpctl")
                        .args(["set-default", idx])
                        .status();
                    break;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
        Ok(self.default_sink_name().as_deref() == Some(name))
    }

    fn teardown_links(&mut self) {
        teardown_buschain_links();
    }

    fn migrate_bus_clock(&mut self, spec: &NodeSpec, clock: &GraphClock) -> Result<()> {
        null_sink::migrate_recreate_null_sink(spec, &ClockProps::from(clock))
    }

    fn teardown_rate_bridges(&mut self) -> Result<()> {
        let sinks = list_devices("sinks")?;
        for s in sinks {
            if s.name.starts_with("buschain_rs_") || s.name.starts_with("shadow_rs_") {
                let _ = null_sink::unload_named_null_sink(&s.name);
            }
        }
        Ok(())
    }
}

fn list_devices(kind: &str) -> Result<Vec<DeviceNode>> {
    let out = std::process::Command::new("pactl")
        .args(["list", "short", kind])
        .output()
        .context("pactl list")?;
    if !out.status.success() {
        return Err(anyhow!("pactl list {kind} failed"));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut v = Vec::new();
    for line in text.lines() {
        let mut parts = line.split('\t');
        let _idx = parts.next();
        let Some(name) = parts.next() else { continue };
        let desc = parts.nth(1).unwrap_or(name);
        v.push(DeviceNode {
            name: name.to_string(),
            description: desc.to_string(),
        });
    }
    Ok(v)
}

fn run_ok(bin: &str, args: &[&str]) -> Result<()> {
    let out = std::process::Command::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("spawn {bin}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "{bin} {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

fn set_node_props_cli(node: &str, props: &Props) -> Result<()> {
    if props.entries.is_empty() {
        return Ok(());
    }
    // pw-cli s <id|name> Props '{ key = "val" ... }'
    let mut body = String::from("{ ");
    for (i, (k, v)) in props.entries.iter().enumerate() {
        if i > 0 {
            body.push(' ');
        }
        body.push_str(k);
        body.push_str(" = \"");
        body.push_str(&v.replace('\"', "\\\""));
        body.push('\"');
    }
    body.push_str(" }");
    let out = std::process::Command::new("pw-cli")
        .args(["s", node, "Props", &body])
        .output()
        .context("pw-cli Props")?;
    if !out.status.success() {
        // Best-effort — some nodes reject props; not fatal for soft_quantum hints.
        return Ok(());
    }
    Ok(())
}

/// Ensure a clocked route, inserting buschain_rs_* when rates differ.
///
/// Rate bridges are **inbound only**: external capture → BusChain bus.
/// BusChain → HW (or any non-BusChain sink) always links directly — PipeWire's
/// device adapter handles output resampling. Bridging master→HW was breaking
/// audible output (orphan `buschain_rs_*` hops that never reached the device).
pub fn ensure_clocked_route(
    backend: &mut dyn AudioBackend,
    source: &str,
    sink: &str,
    clock: &GraphClock,
    desired: &mut DesiredState,
    exclusive: bool,
) -> Result<()> {
    ensure_clocked_route_inner(backend, source, sink, clock, desired, exclusive, false)
}

/// Force-recreate capture hop (Add / unmute-row after teardown).
pub fn ensure_clocked_route_force(
    backend: &mut dyn AudioBackend,
    source: &str,
    sink: &str,
    clock: &GraphClock,
    desired: &mut DesiredState,
    exclusive: bool,
) -> Result<()> {
    ensure_clocked_route_inner(backend, source, sink, clock, desired, exclusive, true)
}

fn ensure_clocked_route_inner(
    backend: &mut dyn AudioBackend,
    source: &str,
    sink: &str,
    clock: &GraphClock,
    desired: &mut DesiredState,
    exclusive: bool,
    force: bool,
) -> Result<()> {
    let src_node = source.strip_suffix(".monitor").unwrap_or(source);
    let dst_node = sink.strip_suffix(".monitor").unwrap_or(sink);
    let src_name = NodeName::new(src_node.to_string());
    let dst_name = NodeName::new(dst_node.to_string());

    let src_rate = effective_rate(backend, src_node, clock);
    let dst_rate = effective_rate(backend, dst_node, clock);

    // Outbound to hardware / non-BusChain: never insert a rate-bridge.
    let inbound_to_shadow = dst_name.is_shadow() && !src_name.is_shadow();
    let want_bridge = inbound_to_shadow
        && src_rate != 0
        && dst_rate != 0
        && src_rate != dst_rate;

    if !want_bridge {
        // Rates match (or unknown): drop leftover pair-specific rate-bridge hops.
        let stale: Vec<String> = desired
            .bridges
            .keys()
            .filter(|bridge| {
                let mon = format!("{bridge}.monitor");
                desired.routes.contains(&(source.to_string(), (*bridge).clone()))
                    || desired.routes.contains(&(mon, sink.to_string()))
            })
            .cloned()
            .collect();
        for bridge in stale {
            let mon = format!("{bridge}.monitor");
            let _ = backend.unlink_raw(&mon, sink);
            let _ = backend.unlink_raw(source, &bridge);
            desired.routes.remove(&(source.to_string(), bridge.clone()));
            desired.routes.remove(&(mon, sink.to_string()));
        }
        if exclusive {
            let _ = backend.unlink_from_source_except(source, &[sink, "buschain_hold"]);
        }
        if force {
            ensure_link_force_pair(source, sink)?;
        } else {
            backend.ensure_link_raw(source, sink)?;
        }
        desired.ensure_route(&LinkSpec {
            source: source.to_string(),
            sink: sink.to_string(),
            exclusive,
        });
        return Ok(());
    }

    // Bridge at GraphClock rate — only the inbound hop runs at the foreign rate.
    let bridge = DesiredState::bridge_name(src_rate, dst_rate, source, sink);
    let bridge_name = bridge.as_str().to_string();
    // Sink-side dual-path prune: never leave dry source→sink beside source→rs→sink.
    // Shared mics stay non-exclusive (no unlink_from_source_except).
    let _ = backend.unlink_raw(source, sink);
    desired.routes.remove(&(source.to_string(), sink.to_string()));
    if exclusive {
        // Do NOT keep a parallel direct source→sink (dual-path chorus).
        let _ = backend.unlink_from_source_except(
            source,
            &[bridge_name.as_str(), "buschain_hold"],
        );
    }
    let spec = NodeSpec {
        name: bridge.clone(),
        description: format!("BusChainControl_RateBridge_{src_rate}_to_{dst_rate}"),
        role: NodeRole::RateBridge,
        start_muted: false,
    };
    backend.ensure_node(&spec, clock)?;
    desired.ensure_bus(spec);
    desired.bridges.insert(bridge_name.clone(), (src_rate, dst_rate));
    // Fresh null-sink ports lag registry/CLI caches — wait before Mic→rs ensure
    // so we prefer native/pw-link over a Pulse fallback storm.
    invalidate_probe_caches();
    let _ = wait_sink_playback_ports(&bridge_name, std::time::Duration::from_millis(200));

    let mon = format!("{bridge_name}.monitor");
    if force {
        // Destroy ghost bridge hop first so Add cannot attach to a dead rs.
        let _ = backend.unlink_raw(source, &bridge_name);
        let _ = backend.unlink_raw(&mon, sink);
        ensure_link_force_pair(source, &bridge_name)?;
        ensure_link_force_pair(&mon, sink)?;
    } else {
        backend.ensure_link_raw(source, &bridge_name)?;
        backend.ensure_link_raw(&mon, sink)?;
    }
    desired.ensure_route(&LinkSpec {
        source: source.to_string(),
        sink: bridge_name.clone(),
        exclusive,
    });
    desired.ensure_route(&LinkSpec {
        source: mon,
        sink: sink.to_string(),
        exclusive: false,
    });
    Ok(())
}

fn effective_rate(backend: &mut dyn AudioBackend, node: &str, clock: &GraphClock) -> u32 {
    let name = NodeName::new(node.to_string());
    if name.is_shadow() && !name.is_rate_bridge() {
        // Prefer live bus rate when present (BindMasterClock soft-props lag).
        if let Some(live) = crate::clock::probe_sink_running_rate(node) {
            return live;
        }
        return clock.sample_rate;
    }
    if name.is_rate_bridge() {
        return clock.sample_rate;
    }
    // External: running rate first (mic/HW), then structured probe.
    crate::clock::probe_source_running_rate(node)
        .or_else(|| crate::clock::probe_sink_running_rate(node))
        .or_else(|| {
            backend
                .probe_endpoint(node)
                .ok()
                .and_then(|c| c.rate)
        })
        .unwrap_or(0)
}

/// Filter-chain conf fragment for clock (inserted into capture/playback props).
pub fn filter_chain_clock_props(clock: &GraphClock) -> String {
    let latency = clock.node_latency_prop();
    format!(
        r#"
        audio.rate            = {rate}
        node.rate             = 1/{rate}
        node.latency          = {latency}
        node.lock-quantum     = {soft_inv}
        node.force-quantum    = {quantum}"#,
        rate = clock.sample_rate,
        latency = latency,
        soft_inv = if clock.soft_quantum { "false" } else { "true" },
        quantum = clock.quantum,
    )
}

/// Clock fragment aligned to a live sink's running rate (bus/post), not Custom.
pub fn filter_chain_clock_for_sink(clock: &GraphClock, sink_name: &str) -> (GraphClock, String) {
    let mut aligned = clock.clone();
    if let Some(rate) = probe_sink_running_rate(sink_name) {
        if rate > 0 {
            aligned.sample_rate = rate;
        }
    }
    let frag = filter_chain_clock_props(&aligned);
    (aligned, frag)
}
