//! Native PipeWire control plane (Phase F).
//!
//! One shared MainLoop thread owns Core + Registry; workers read an
//! `RwLock<GraphView>` for probes and send RPCs for mutations.

mod cache;
mod ports;
mod props;
mod session;

use anyhow::{anyhow, Context, Result};

use crate::clock::{probe_endpoint_caps, EndpointCaps, GraphClock};
use crate::contract::ClockProps;
use crate::domain::{
    DeviceNode, GraphSnapshot, LinkId, NodeId, NodeRole, NodeSpec, Props,
};

use super::{gate_bus_monitor, AudioBackend};

/// List MIDI nodes from the native PipeWire registry (empty when plane is cold).
pub fn list_midi_nodes() -> Vec<(String, String)> {
    let Some(view) = session::shared_view() else {
        return Vec::new();
    };
    let Ok(g) = view.read() else {
        return Vec::new();
    };
    g.devices_of_class("Midi")
}

pub use session::{
    cache_node_id as native_cache_node_id, ensure_link as native_ensure_link,
    ensure_null_sink as session_ensure_null_sink, find_node_id as native_find_node_id,
    is_ready as native_ready, link_is_live as native_link_is_live,
    set_levels as native_set_levels, sink_exists as native_sink_exists, unlink as native_unlink,
};

/// Default engine backend: native registry/links/sinks; Pulse CLI for levels.
pub struct PipewireNativeBackend;

impl Default for PipewireNativeBackend {
    fn default() -> Self {
        Self
    }
}

impl PipewireNativeBackend {
    pub fn new() -> Self {
        // Touch the control plane early so Apply does not pay startup latency.
        let _ = session::is_ready();
        Self
    }
}

impl AudioBackend for PipewireNativeBackend {
    fn snapshot(&mut self) -> Result<GraphSnapshot> {
        if session::is_ready() {
            let sinks: Vec<DeviceNode> = session::list_sink_names()
                .into_iter()
                .map(|name| {
                    let description = session::shared_view()
                        .and_then(|v| {
                            v.read()
                                .ok()
                                .and_then(|g| g.node(&name).map(|n| n.description.clone()))
                        })
                        .unwrap_or_else(|| name.clone());
                    DeviceNode { name, description }
                })
                .collect();
            let sources: Vec<DeviceNode> = session::list_source_devices()
                .into_iter()
                .map(|(name, description)| DeviceNode { name, description })
                .collect();
            if !sinks.is_empty() || !sources.is_empty() {
                return Ok(GraphSnapshot { sinks, sources });
            }
        }
        Ok(GraphSnapshot {
            sinks: list_devices_cli("sinks")?,
            sources: list_devices_cli("sources")?,
        })
    }

    fn ensure_node(&mut self, spec: &NodeSpec, clock: &GraphClock) -> Result<NodeId> {
        let clock = ClockProps::from(clock);
        if session::is_ready() {
            // Match CLI ensure semantics for app buses: soft props if present.
            if session::sink_exists(spec.name.as_str()) {
                let is_app = matches!(spec.role, NodeRole::TrackBus | NodeRole::MasterBus);
                if is_app {
                    return Ok(NodeId(spec.name.0.clone()));
                }
                if session::shared_view()
                    .and_then(|v| v.read().ok().map(|g| g.null_sink_ready(spec.name.as_str())))
                    .unwrap_or(false)
                {
                    return Ok(NodeId(spec.name.0.clone()));
                }
                let _ = session::destroy_node(spec.name.as_str());
            }
            session::ensure_null_sink(spec, &clock)?;
            let is_app = matches!(spec.role, NodeRole::TrackBus | NodeRole::MasterBus);
            if is_app || spec.start_muted {
                let _ = session::set_levels(spec.name.as_str(), -120.0, true);
            }
            return Ok(NodeId(spec.name.0.clone()));
        }
        super::null_sink::ensure_null_sink(spec, &clock)?;
        Ok(NodeId(spec.name.0.clone()))
    }

    fn destroy_node(&mut self, name: &str) -> Result<()> {
        if session::is_ready() {
            match session::destroy_node(name) {
                Ok(()) => return Ok(()),
                Err(_) => {
                    // Fall through to Pulse module unload for pactl-created sinks.
                }
            }
        }
        super::null_sink::unload_named_null_sink(name)
    }

    fn ensure_link_raw(&mut self, source: &str, sink: &str) -> Result<LinkId> {
        if session::is_ready() {
            match session::ensure_link(source, sink) {
                Ok(()) => return Ok(LinkId(format!("{source}->{sink}"))),
                Err(e) => {
                    // Host FX: never Pulse-loopback.
                    let dst = ports::pw_node(sink);
                    if dst.starts_with("buschain_fx_")
                        || ports::pw_node(source).starts_with("buschain_fx_")
                    {
                        return Err(e);
                    }
                    // Foreign / race: CLI fallback once.
                    super::link::ensure_link(source, sink)?;
                    return Ok(LinkId(format!("{source}->{sink}")));
                }
            }
        }
        super::link::ensure_link(source, sink)?;
        Ok(LinkId(format!("{source}->{sink}")))
    }

    fn unlink_raw(&mut self, source: &str, sink: &str) -> Result<()> {
        if session::is_ready() {
            let _ = session::unlink(source, sink);
        }
        super::link::unlink(source, sink)
    }

    fn unlink_from_source_except(&mut self, source: &str, allow_sinks: &[&str]) -> u32 {
        let mut n = 0;
        if session::is_ready() {
            n += session::unlink_from_source_except(source, allow_sinks);
        }
        n + super::link::unlink_from_source_except(source, allow_sinks)
    }

    fn set_props(&mut self, node: &str, props: &Props) -> Result<()> {
        if session::is_ready() {
            match session::set_node_props(node, props.entries.clone()) {
                Ok(()) => return Ok(()),
                Err(_) => {}
            }
        }
        set_node_props_cli(node, props)
    }

    fn probe_endpoint(&mut self, name: &str) -> Result<EndpointCaps> {
        if let Some(view) = session::shared_view() {
            if let Ok(g) = view.read() {
                if let Some(n) = g.node(name) {
                    if let Some(rate) = n.rate {
                        let mut caps = probe_endpoint_caps(name);
                        caps.rate = Some(rate);
                        return Ok(caps);
                    }
                }
            }
        }
        Ok(probe_endpoint_caps(name))
    }

    fn set_levels(&mut self, sink: &str, gain_db: f32, muted: bool) -> Result<()> {
        if session::is_ready() {
            match session::set_levels(sink, gain_db, muted) {
                Ok(()) => return Ok(()),
                Err(_) => {}
            }
        }
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
        self.set_levels(sink, gain_db, false)
    }

    fn gate_monitor(&mut self, bus: &str, gated: bool) -> Result<()> {
        let mon = format!("{bus}.monitor");
        if session::is_ready() {
            // Gate = mute + 0 volume on monitor source (never mute the app sink).
            match session::set_levels(&mon, if gated { -120.0 } else { 0.0 }, gated) {
                Ok(()) => return Ok(()),
                Err(_) => {}
            }
        }
        gate_bus_monitor(bus, gated)
    }

    fn list_sink_names(&mut self) -> Result<Vec<String>> {
        if session::is_ready() {
            let names = session::list_sink_names();
            if !names.is_empty() {
                return Ok(names);
            }
        }
        Ok(list_devices_cli("sinks")?
            .into_iter()
            .map(|d| d.name)
            .collect())
    }

    fn default_sink_name(&mut self) -> Option<String> {
        if let Some(n) = session::default_sink_name() {
            return Some(n);
        }
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
        if session::is_ready() {
            if let Ok(true) = session::set_default_sink(name) {
                return Ok(true);
            }
        }
        match run_ok("pactl", &["set-default-sink", name]) {
            Ok(()) => {
                std::thread::sleep(std::time::Duration::from_millis(40));
                Ok(self.default_sink_name().as_deref() == Some(name))
            }
            Err(_) => Ok(false),
        }
    }

    fn teardown_links(&mut self) {
        super::link::teardown_buschain_links();
    }

    fn migrate_bus_clock(&mut self, spec: &NodeSpec, clock: &GraphClock) -> Result<()> {
        // Stream move still needs Pulse; recreate node via native when possible.
        super::null_sink::migrate_recreate_null_sink(spec, &ClockProps::from(clock))
    }

    fn teardown_rate_bridges(&mut self) -> Result<()> {
        let names = self.list_sink_names()?;
        for name in names {
            if name.starts_with("buschain_rs_") || name.starts_with("shadow_rs_") {
                let _ = self.destroy_node(&name);
            }
        }
        Ok(())
    }
}

fn list_devices_cli(kind: &str) -> Result<Vec<DeviceNode>> {
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
    let _ = std::process::Command::new("pw-cli")
        .args(["s", node, "Props", &body])
        .output();
    Ok(())
}
