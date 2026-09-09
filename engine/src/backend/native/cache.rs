//! Live registry snapshot shared with worker threads (read-mostly).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDir {
    In,
    Out,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct NodeRec {
    pub id: u32,
    pub name: String,
    pub media_class: String,
    pub description: String,
    pub rate: Option<u32>,
    /// PipeWire `object.serial` — matches Pulse sink-input / sink index.
    pub serial: Option<u32>,
}

/// Application props captured for `Stream/Output/Audio` nodes (Apps discovery).
#[derive(Debug, Clone, Default)]
pub struct StreamProps {
    pub app_name: Option<String>,
    pub binary: Option<String>,
    pub app_id: Option<String>,
    pub media_name: Option<String>,
    pub icon_name: Option<String>,
    /// `media.role` — anonymous event/notify (System Sounds) stay out of reclaim;
    /// app-owned event streams still reclaim when they have real identity.
    pub media_role: Option<String>,
    pub node_virtual: bool,
}

#[derive(Debug, Clone)]
pub struct PortRec {
    pub id: u32,
    pub node_id: u32,
    pub name: String,
    pub direction: PortDir,
    pub channel: String,
}

#[derive(Debug, Clone)]
pub struct LinkRec {
    pub id: u32,
    pub out_node: u32,
    pub out_port: u32,
    pub in_node: u32,
    pub in_port: u32,
}

#[derive(Debug, Default, Clone)]
pub struct GraphView {
    pub generation: u64,
    pub nodes_by_id: HashMap<u32, NodeRec>,
    pub nodes_by_name: HashMap<String, u32>,
    pub ports_by_id: HashMap<u32, PortRec>,
    pub ports_by_node: HashMap<u32, Vec<u32>>,
    pub links_by_id: HashMap<u32, LinkRec>,
    /// (out_port_id, in_port_id) → link global id
    pub links_by_ports: HashMap<(u32, u32), u32>,
    /// node id → application props (Stream/Output/Audio only).
    pub stream_props_by_id: HashMap<u32, StreamProps>,
    pub link_factory: Option<String>,
    /// Cached `default.audio.sink` name from Metadata.
    pub default_audio_sink: Option<String>,
}

impl GraphView {
    pub fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Wipe registry cache (PipeWire daemon died / control plane reconnect).
    pub fn clear(&mut self) {
        *self = Self::default();
        self.bump();
    }

    pub fn node_id(&self, name: &str) -> Option<u32> {
        self.nodes_by_name.get(name).copied()
    }

    pub fn node(&self, name: &str) -> Option<&NodeRec> {
        let id = self.node_id(name)?;
        self.nodes_by_id.get(&id)
    }

    pub fn sink_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .nodes_by_id
            .values()
            .filter(|n| {
                n.media_class == "Audio/Sink"
                    || n.media_class.ends_with("/Sink")
                    || n.name.starts_with("buschain_")
            })
            .map(|n| n.name.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Pulse-exported sinks only (exact `Audio/Sink`) — Internal helpers omitted.
    pub fn pulse_sink_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .nodes_by_id
            .values()
            .filter(|n| n.media_class == "Audio/Sink")
            .map(|n| n.name.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn node_by_serial(&self, serial: u32) -> Option<&NodeRec> {
        self.nodes_by_id.values().find(|n| n.serial == Some(serial))
    }

    pub fn source_names(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .nodes_by_id
            .values()
            .filter(|n| {
                n.media_class == "Audio/Source"
                    || n.media_class.ends_with("/Source")
                    || n.media_class.contains("Source")
            })
            .map(|n| (n.name.clone(), n.description.clone()))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v.dedup_by(|a, b| a.0 == b.0);
        v
    }

    pub fn devices_of_class(&self, class_substr: &str) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .nodes_by_id
            .values()
            .filter(|n| n.media_class.contains(class_substr))
            .map(|n| (n.name.clone(), n.description.clone()))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v.dedup_by(|a, b| a.0 == b.0);
        v
    }

    pub fn ports_of(&self, node_id: u32) -> Vec<&PortRec> {
        self.ports_by_node
            .get(&node_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.ports_by_id.get(id))
            .collect()
    }

    pub fn link_exists(&self, out_port: u32, in_port: u32) -> bool {
        self.links_by_ports.contains_key(&(out_port, in_port))
    }

    /// True when node has at least one playback (in) and one monitor (out) port.
    pub fn null_sink_ready(&self, name: &str) -> bool {
        let Some(id) = self.node_id(name) else {
            return false;
        };
        let ports = self.ports_of(id);
        let has_play = ports.iter().any(|p| {
            p.direction == PortDir::In
                && (p.name.to_lowercase().contains("playback")
                    || p.name.to_lowercase().contains("input"))
        });
        let has_mon = ports.iter().any(|p| {
            p.direction == PortDir::Out && p.name.to_lowercase().contains("monitor")
        });
        // Duplex FX filters expose input_* / output_* instead of playback/monitor.
        let has_in = ports.iter().any(|p| p.direction == PortDir::In);
        let has_out = ports.iter().any(|p| p.direction == PortDir::Out);
        (has_play && has_mon) || (has_in && has_out && ports.len() >= 2)
    }

    pub fn insert_node(&mut self, rec: NodeRec) {
        if !rec.name.is_empty() {
            self.nodes_by_name.insert(rec.name.clone(), rec.id);
        }
        self.nodes_by_id.insert(rec.id, rec);
        self.bump();
    }

    pub fn insert_stream_props(&mut self, node_id: u32, props: StreamProps) {
        self.stream_props_by_id.insert(node_id, props);
        self.bump();
    }

    pub fn has_global(&self, id: u32) -> bool {
        self.nodes_by_id.contains_key(&id)
            || self.links_by_id.contains_key(&id)
            || self.ports_by_id.contains_key(&id)
    }

    pub fn remove_global(&mut self, id: u32) {
        if let Some(n) = self.nodes_by_id.remove(&id) {
            if self.nodes_by_name.get(&n.name) == Some(&id) {
                self.nodes_by_name.remove(&n.name);
            }
        }
        self.stream_props_by_id.remove(&id);
        if self.ports_by_id.remove(&id).is_some() {
            for ports in self.ports_by_node.values_mut() {
                ports.retain(|p| *p != id);
            }
        }
        if let Some(link) = self.links_by_id.remove(&id) {
            self.links_by_ports
                .remove(&(link.out_port, link.in_port));
        }
        self.bump();
    }

    pub fn insert_port(&mut self, rec: PortRec) {
        self.ports_by_node
            .entry(rec.node_id)
            .or_default()
            .push(rec.id);
        if let Some(ports) = self.ports_by_node.get_mut(&rec.node_id) {
            ports.sort_unstable();
            ports.dedup();
        }
        self.ports_by_id.insert(rec.id, rec);
        self.bump();
    }

    pub fn insert_link(&mut self, rec: LinkRec) {
        self.links_by_ports
            .insert((rec.out_port, rec.in_port), rec.id);
        self.links_by_id.insert(rec.id, rec);
        self.bump();
    }
}

pub type SharedView = Arc<RwLock<GraphView>>;

pub fn new_shared() -> SharedView {
    Arc::new(RwLock::new(GraphView::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_sink_ready_needs_monitor_and_playback() {
        let mut g = GraphView::default();
        g.insert_node(NodeRec {
            id: 1,
            name: "buschain_post_x".into(),
            media_class: "Audio/Sink".into(),
            description: String::new(),
            rate: Some(48_000),
            serial: None,
        });
        assert!(!g.null_sink_ready("buschain_post_x"));
        g.insert_port(PortRec {
            id: 10,
            node_id: 1,
            name: "playback_FL".into(),
            direction: PortDir::In,
            channel: "FL".into(),
        });
        g.insert_port(PortRec {
            id: 11,
            node_id: 1,
            name: "monitor_FL".into(),
            direction: PortDir::Out,
            channel: "FL".into(),
        });
        assert!(g.null_sink_ready("buschain_post_x"));
    }
}
