//! AudioBackend trait + PipeWire CLI implementation.

mod cli;
mod fx_chain;
mod link;
mod null_sink;

pub use cli::invalidate_probe_caches;

pub use fx_chain::{
    find_node_id_by_name, push_insert_controls, read_signature, sink_exists, sink_has_input,
    spawn_sidechain, FilterChainRuntime,
};

pub use link::{ensure_link, link_is_live, teardown_buschain_links, unlink};

use anyhow::{anyhow, Context, Result};

use crate::clock::{probe_endpoint_caps, probe_sink_running_rate, EndpointCaps, GraphClock};
use crate::contract::ClockProps;
use crate::domain::{
    DeviceNode, GraphSnapshot, LinkId, LinkSpec, NodeId, NodeName, NodeRole, NodeSpec, Props,
};
use crate::plan::DesiredState;

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
        ensure_link(source, sink)?;
        Ok(LinkId(format!("{source}->{sink}")))
    }

    fn unlink_raw(&mut self, source: &str, sink: &str) -> Result<()> {
        unlink(source, sink)
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
            if s.name.starts_with("buschain_rs_") {
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
        if exclusive {
            let _ = backend.unlink_from_source_except(source, &[sink, "buschain_hold"]);
        }
        backend.ensure_link_raw(source, sink)?;
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

    backend.ensure_link_raw(source, &bridge_name)?;
    let mon = format!("{bridge_name}.monitor");
    backend.ensure_link_raw(&mon, sink)?;
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
