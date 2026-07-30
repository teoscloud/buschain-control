//! LV2 `AudioProcessor` via lilv (optional — enable `lv2-host` / `dsp-host`).
//!
//! Off-RT: load bundle → instantiate → connect stereo audio + control ports → activate.
//! RT: planar stereo `run()` with ControlQueue → control buffer updates.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

use super::processor::AudioProcessor;

/// One discovered LV2 control port.
#[derive(Debug, Clone)]
pub struct Lv2ParamInfo {
    pub index: usize,
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub toggled: bool,
}

struct PortMap {
    audio_in: Vec<usize>,
    audio_out: Vec<usize>,
    control_in: Vec<usize>,
    control_names: Vec<String>,
}

#[cfg(feature = "lv2-host")]
mod imp {
    use super::*;
    use lilv::instance::{ActiveInstance, Instance};
    use lilv::node::Node;
    use lilv::plugin::Plugin;
    use lilv::World;

    fn lv2_uri(world: &World, fragment: &str) -> Node {
        world.new_uri(&format!("http://lv2plug.in/ns/lv2core#{fragment}"))
    }

    fn bundle_file_uri(world: &World, path: &str) -> Result<Node> {
        let abs = Path::new(path)
            .canonicalize()
            .with_context(|| format!("LV2 bundle path {path}"))?;
        let s = abs.display().to_string();
        Ok(world.new_file_uri(None, &s))
    }

    fn find_plugin<'w>(
        world: &'w World,
        uri: &str,
        bundle_path: Option<&str>,
    ) -> Result<Plugin> {
        if let Some(bundle) = bundle_path.filter(|p| !p.is_empty()) {
            let bundle_uri = bundle_file_uri(world, bundle)?;
            world.load_bundle(&bundle_uri);
            world.load_plugin_classes();
        } else {
            world.load_all();
        }

        if !uri.is_empty() {
            let uri_node = world.new_uri(uri);
            if let Some(p) = world.plugins().plugin(&uri_node) {
                return Ok(p);
            }
            bail!("LV2 plugin URI not found: {uri}");
        }

        // Bundle-only id — pick the first plugin in the loaded bundle.
        if let Some(bundle) = bundle_path.filter(|p| !p.is_empty()) {
            let bundle_uri = bundle_file_uri(world, bundle)?;
            let bundle_str = bundle_uri.as_uri().unwrap_or("");
            for p in world.plugins().iter() {
                if p.bundle_uri().as_uri().unwrap_or("") == bundle_str {
                    return Ok(p);
                }
            }
        }

        bail!("no LV2 plugin in bundle {:?}", bundle_path)
    }

    fn map_ports(world: &World, plugin: &Plugin) -> Result<PortMap> {
        let input = lv2_uri(world, "InputPort");
        let output = lv2_uri(world, "OutputPort");
        let audio = lv2_uri(world, "AudioPort");
        let control = lv2_uri(world, "ControlPort");

        let mut audio_in = Vec::new();
        let mut audio_out = Vec::new();
        let mut control_in = Vec::new();
        let mut control_names = Vec::new();

        for port in plugin.iter_ports() {
            let idx = port.index();
            let name = port
                .name()
                .and_then(|n| n.as_str().map(str::to_string))
                .or_else(|| {
                    port.symbol()
                        .and_then(|s| s.as_str().map(str::to_string))
                })
                .unwrap_or_else(|| format!("port{idx}"));

            if port.is_a(&control) && port.is_a(&input) {
                control_in.push(idx);
                control_names.push(name);
            } else if port.is_a(&audio) && port.is_a(&input) {
                audio_in.push(idx);
            } else if port.is_a(&audio) && port.is_a(&output) {
                audio_out.push(idx);
            }
        }

        if audio_in.is_empty() || audio_out.is_empty() {
            bail!("LV2 plugin has no stereo audio in/out ports");
        }
        Ok(PortMap {
            audio_in,
            audio_out,
            control_in,
            control_names,
        })
    }

    fn discover_params(world: &World, plugin: &Plugin, ports: &PortMap) -> Vec<Lv2ParamInfo> {
        let toggled = lv2_uri(world, "toggled");
        let ranges = plugin.port_ranges_float();
        let mut out = Vec::new();
        for (ci, &port_idx) in ports.control_in.iter().enumerate() {
            let Some(port) = plugin.port_by_index(port_idx) else {
                continue;
            };
            let name = ports
                .control_names
                .get(ci)
                .cloned()
                .unwrap_or_else(|| format!("ctrl{ci}"));
            let pr = ranges.get(port_idx).copied().unwrap_or(lilv::port::FloatRanges {
                default: 0.0,
                min: 0.0,
                max: 1.0,
            });
            out.push(Lv2ParamInfo {
                index: ci,
                name,
                min: pr.min,
                max: pr.max,
                default: pr.default,
                toggled: port.has_property(&toggled),
            });
        }
        out
    }

    struct LiveLv2 {
        active: ActiveInstance,
        audio_in: Vec<usize>,
        audio_out: Vec<usize>,
        in_l: Vec<f32>,
        in_r: Vec<f32>,
        out_l: Vec<f32>,
        out_r: Vec<f32>,
    }

    /// Activated LV2 insert.
    pub struct Lv2Instance {
        _world: World,
        plugin_uri: String,
        params: Vec<Lv2ParamInfo>,
        control_values: Vec<f32>,
        ports: PortMap,
        latency: u32,
        sample_rate: u32,
        max_block: u32,
        live: Option<LiveLv2>,
    }

    impl Lv2Instance {
        pub fn load(
            uri: &str,
            bundle_path: &str,
            sample_rate: u32,
            max_block: u32,
        ) -> Result<Self> {
            if sample_rate == 0 || max_block == 0 {
                bail!("invalid audio config sr={sample_rate} max_block={max_block}");
            }

            let world = World::new();
            let bundle = bundle_path
                .is_empty()
                .then(|| None)
                .unwrap_or(Some(bundle_path));
            let plugin = find_plugin(&world, uri, bundle)?;
            if !plugin.verify() {
                bail!("LV2 plugin failed verify: {uri}");
            }

            let ports = map_ports(&world, &plugin)?;
            let params = discover_params(&world, &plugin, &ports);
            let control_values: Vec<f32> = params.iter().map(|p| p.default).collect();

            // SAFETY: off-RT instantiate + activate per LV2 spec.
            let instance = unsafe { plugin.instantiate(sample_rate as f64, std::iter::empty()) }
                .ok_or_else(|| anyhow!("LV2 instantiate failed: {uri}"))?;

            let mut inst = Self {
                _world: world,
                plugin_uri: if uri.is_empty() {
                    plugin.uri().as_uri().unwrap_or("").to_string()
                } else {
                    uri.to_string()
                },
                params,
                control_values,
                ports,
                latency: plugin
                    .latency_port_index()
                    .map(|_| 0)
                    .unwrap_or(0),
                sample_rate,
                max_block: max_block.max(1),
                live: None,
            };

            inst.connect_and_activate(instance)?;
            Ok(inst)
        }

        fn connect_and_activate(&mut self, mut instance: Instance) -> Result<()> {
            let n = self.max_block as usize;
            let in_l = vec![0.0f32; n];
            let in_r = vec![0.0f32; n];
            let mut out_l = vec![0.0f32; n];
            let mut out_r = vec![0.0f32; n];

            // Connect controls
            for (ci, &port_idx) in self.ports.control_in.iter().enumerate() {
                unsafe {
                    instance.connect_port_mut(port_idx, self.control_values.as_mut_ptr().add(ci));
                }
            }

            // Stereo audio — first two in/out audio ports.
            let ai0 = self.ports.audio_in[0];
            let ai1 = self.ports.audio_in.get(1).copied().unwrap_or(ai0);
            let ao0 = self.ports.audio_out[0];
            let ao1 = self.ports.audio_out.get(1).copied().unwrap_or(ao0);

            unsafe {
                instance.connect_port(ai0, in_l.as_ptr());
                instance.connect_port(ai1, in_r.as_ptr());
                instance.connect_port_mut(ao0, out_l.as_mut_ptr());
                instance.connect_port_mut(ao1, out_r.as_mut_ptr());
            }

            let active = unsafe { instance.activate() };
            self.live = Some(LiveLv2 {
                active,
                audio_in: vec![ai0, ai1],
                audio_out: vec![ao0, ao1],
                in_l,
                in_r,
                out_l,
                out_r,
            });
            Ok(())
        }

        pub fn plugin_uri(&self) -> &str {
            &self.plugin_uri
        }

        pub fn discovered_params(&self) -> &[Lv2ParamInfo] {
            &self.params
        }
    }

    impl AudioProcessor for Lv2Instance {
        fn prepare(&mut self, _sample_rate: u32, max_block: u32) {
            let n = max_block.max(1) as usize;
            if let Some(live) = self.live.as_mut() {
                if live.in_l.len() < n {
                    live.in_l.resize(n, 0.0);
                    live.in_r.resize(n, 0.0);
                    live.out_l.resize(n, 0.0);
                    live.out_r.resize(n, 0.0);
                }
            }
            self.max_block = max_block.max(1);
        }

        fn latency_samples(&self) -> u32 {
            self.latency
        }

        fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
            let n = left.len().min(right.len());
            if n == 0 {
                return;
            }
            let Some(live) = self.live.as_mut() else {
                return;
            };
            if n > live.in_l.len() {
                return;
            }

            live.in_l[..n].copy_from_slice(&left[..n]);
            live.in_r[..n].copy_from_slice(&right[..n]);

            unsafe {
                live.active.run(n);
            }

            left[..n].copy_from_slice(&live.out_l[..n]);
            right[..n].copy_from_slice(&live.out_r[..n]);
        }

        fn set_control(&mut self, index: usize, value: f32) {
            if let Some(p) = self.params.get(index) {
                let v = value.clamp(p.min, p.max);
                if let Some(slot) = self.control_values.get_mut(index) {
                    *slot = v;
                }
            }
        }

        fn control_count(&self) -> usize {
            self.params.len()
        }

        fn control_name(&self, index: usize) -> Option<&str> {
            self.params.get(index).map(|p| p.name.as_str())
        }
    }

    pub fn probe_lv2_params(uri: &str, bundle_path: Option<&str>) -> Result<Vec<Lv2ParamInfo>> {
        let world = World::new();
        let plugin = find_plugin(&world, uri, bundle_path)?;
        let ports = map_ports(&world, &plugin)?;
        Ok(discover_params(&world, &plugin, &ports))
    }

    pub fn apply_controls(inst: &mut Lv2Instance, controls: &[(String, f32)]) {
        for (name, val) in controls {
            let lname = name.to_ascii_lowercase();
            if lname == "bypass" || lname == "enable" {
                continue;
            }
            if let Some(idx) = inst
                .params
                .iter()
                .position(|p| p.name.eq_ignore_ascii_case(name))
            {
                inst.set_control(idx, *val);
            }
        }
    }
}

#[cfg(not(feature = "lv2-host"))]
mod imp {
    use super::*;

    pub struct Lv2Instance;

    impl Lv2Instance {
        pub fn load(_uri: &str, _bundle_path: &str, _sample_rate: u32, _max_block: u32) -> Result<Self> {
            bail!("LV2 host not compiled — rebuild with feature lv2-host (lilv/pkg-config)")
        }
    }

    pub fn probe_lv2_params(_uri: &str, _bundle_path: Option<&str>) -> Result<Vec<Lv2ParamInfo>> {
        bail!("LV2 host not compiled — rebuild with feature lv2-host (lilv/pkg-config)")
    }

    pub fn apply_controls(_inst: &mut Lv2Instance, _controls: &[(String, f32)]) {}
}

pub use imp::{apply_controls, probe_lv2_params, Lv2Instance};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_host_errors_cleanly_without_lilv() {
        #[cfg(not(feature = "lv2-host"))]
        assert!(Lv2Instance::load("urn:test", "", 48000, 256).is_err());
    }
}
