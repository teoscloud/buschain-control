//! Resolve Pulse-style endpoints to PipeWire port id pairs from the registry cache.

use anyhow::{anyhow, Result};

use super::cache::{GraphView, PortDir, PortRec};

/// Strip Pulse `.monitor` suffix → PipeWire node name.
pub fn pw_node(endpoint: &str) -> &str {
    endpoint.strip_suffix(".monitor").unwrap_or(endpoint)
}

fn is_monitor_source(source: &str) -> bool {
    source.ends_with(".monitor")
}

fn channel_is_fl(ch: &str, name: &str) -> bool {
    let pl = format!("{ch}:{name}").to_lowercase();
    pl.contains("fl")
        || pl.contains("front-left")
        || pl.contains("_0")
        || pl.ends_with(":0")
        || ch.eq_ignore_ascii_case("FL")
}

fn channel_is_fr(ch: &str, name: &str) -> bool {
    let pl = format!("{ch}:{name}").to_lowercase();
    pl.contains("fr")
        || pl.contains("front-right")
        || pl.contains("_1")
        || pl.ends_with(":1")
        || ch.eq_ignore_ascii_case("FR")
}

fn pick_lr<'a>(ports: &[&'a PortRec], prefer_monitor: bool) -> Option<(&'a PortRec, &'a PortRec)> {
    let mut fl = None;
    let mut fr = None;
    for p in ports {
        let pl = p.name.to_lowercase();
        if prefer_monitor && !pl.contains("monitor") && (pl.contains("playback") || pl.contains("input"))
        {
            continue;
        }
        if !prefer_monitor && pl.contains("monitor") {
            continue;
        }
        if channel_is_fl(&p.channel, &p.name) && fl.is_none() {
            fl = Some(*p);
        } else if channel_is_fr(&p.channel, &p.name) && fr.is_none() {
            fr = Some(*p);
        }
    }
    if fl.is_none() || fr.is_none() {
        if ports.len() >= 2 {
            return Some((ports[0], ports[1]));
        }
        return None;
    }
    Some((fl.unwrap(), fr.unwrap()))
}

/// Stereo (out_port_id, in_port_id) pairs for Pulse `source` → `sink`.
pub fn port_id_pairs(view: &GraphView, source: &str, sink: &str) -> Result<Vec<(u32, u32)>> {
    let src_name = pw_node(source);
    let dst_name = pw_node(sink);
    let src_id = view
        .node_id(src_name)
        .ok_or_else(|| anyhow!("no PW node `{src_name}` (from `{source}`)"))?;
    let dst_id = view
        .node_id(dst_name)
        .ok_or_else(|| anyhow!("no PW node `{dst_name}` (from `{sink}`)"))?;

    let src_ports: Vec<&PortRec> = view
        .ports_of(src_id)
        .into_iter()
        .filter(|p| p.direction == PortDir::Out || p.direction == PortDir::Unknown)
        .collect();
    let dst_ports: Vec<&PortRec> = view
        .ports_of(dst_id)
        .into_iter()
        .filter(|p| p.direction == PortDir::In || p.direction == PortDir::Unknown)
        .collect();

    if src_ports.is_empty() {
        return Err(anyhow!("no PW output ports for node `{src_name}`"));
    }
    if dst_ports.is_empty() {
        return Err(anyhow!("no PW input ports for node `{dst_name}`"));
    }

    let prefer_src_monitor =
        is_monitor_source(source) || src_ports.iter().any(|p| p.name.contains("monitor"));
    let (s_fl, s_fr) = pick_lr(&src_ports, prefer_src_monitor)
        .ok_or_else(|| anyhow!("could not pick L/R outputs on `{src_name}`"))?;
    let (d_fl, d_fr) = pick_lr(&dst_ports, false)
        .ok_or_else(|| anyhow!("could not pick L/R inputs on `{dst_name}`"))?;

    Ok(vec![(s_fl.id, d_fl.id), (s_fr.id, d_fr.id)])
}

pub fn link_is_live(view: &GraphView, source: &str, sink: &str) -> bool {
    let Ok(pairs) = port_id_pairs(view, source, sink) else {
        return false;
    };
    pairs
        .iter()
        .all(|(o, i)| view.link_exists(*o, *i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::native::cache::{LinkRec, NodeRec, PortDir, PortRec};

    fn stereo_bus(view: &mut GraphView, id: u32, name: &str) {
        view.insert_node(NodeRec {
            id,
            name: name.into(),
            media_class: "Audio/Sink".into(),
            description: String::new(),
            rate: None,
        });
        view.insert_port(PortRec {
            id: id * 10,
            node_id: id,
            name: "monitor_FL".into(),
            direction: PortDir::Out,
            channel: "FL".into(),
        });
        view.insert_port(PortRec {
            id: id * 10 + 1,
            node_id: id,
            name: "monitor_FR".into(),
            direction: PortDir::Out,
            channel: "FR".into(),
        });
        view.insert_port(PortRec {
            id: id * 10 + 2,
            node_id: id,
            name: "playback_FL".into(),
            direction: PortDir::In,
            channel: "FL".into(),
        });
        view.insert_port(PortRec {
            id: id * 10 + 3,
            node_id: id,
            name: "playback_FR".into(),
            direction: PortDir::In,
            channel: "FR".into(),
        });
    }

    #[test]
    fn pulse_monitor_maps_to_monitor_ports() {
        let mut g = GraphView::default();
        stereo_bus(&mut g, 1, "buschain_track_a");
        stereo_bus(&mut g, 2, "buschain_fx_a");
        let pairs = port_id_pairs(&g, "buschain_track_a.monitor", "buschain_fx_a").unwrap();
        assert_eq!(pairs, vec![(10, 22), (11, 23)]);
        g.insert_link(LinkRec {
            id: 99,
            out_node: 1,
            out_port: 10,
            in_node: 2,
            in_port: 22,
        });
        g.insert_link(LinkRec {
            id: 100,
            out_node: 1,
            out_port: 11,
            in_node: 2,
            in_port: 23,
        });
        assert!(link_is_live(&g, "buschain_track_a.monitor", "buschain_fx_a"));
    }
}
