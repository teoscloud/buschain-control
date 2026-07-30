use super::{
    clap_stub::ClapBackend, ladspa::LadspaBackend, lv2::Lv2Backend, PluginBackend, PluginDescriptor,
    PluginFormat,
};

pub struct PluginHost {
    backends: Vec<Box<dyn PluginBackend>>,
    cache: Vec<PluginDescriptor>,
}

impl PluginHost {
    pub fn new(ladspa_paths: &[String], lv2_paths: &[String], clap_paths: &[String], vst3: bool) -> Self {
        let mut backends: Vec<Box<dyn PluginBackend>> = vec![
            Box::new(LadspaBackend::new(ladspa_paths.to_vec())),
            Box::new(Lv2Backend::new(lv2_paths.to_vec())),
            Box::new(ClapBackend::new(clap_paths.to_vec())),
        ];
        if vst3 {
            backends.push(Box::new(super::vst3_carla::CarlaVst3Backend::new()));
        }
        let mut host = Self {
            backends,
            cache: vec![],
        };
        host.rescan();
        host
    }

    pub fn rescan(&mut self) {
        self.cache.clear();
        for b in &self.backends {
            self.cache.extend(b.scan());
        }
        self.cache.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    }

    pub fn plugins(&self) -> &[PluginDescriptor] {
        &self.cache
    }

    pub fn by_format(&self, format: PluginFormat) -> impl Iterator<Item = &PluginDescriptor> {
        self.cache.iter().filter(move |p| p.id.format == format)
    }

    pub fn builtins(&self) -> Vec<&PluginDescriptor> {
        self.cache
            .iter()
            .filter(|p| {
                p.id.id.contains("buschain_")
                    || p.name.to_lowercase().contains("buschain")
            })
            .collect()
    }

    /// Descriptors that can be added from the Mixer (rack formats today).
    /// LADSPA / CLAP / LV2 always; VST3 when `vst3_enabled`.
    pub fn plugins_for_mixer_add(&self, vst3_enabled: bool) -> Vec<&PluginDescriptor> {
        self.cache
            .iter()
            .filter(|p| match p.id.format {
                PluginFormat::Ladspa => {
                    let id = p.id.id.to_lowercase();
                    let name = p.name.to_lowercase();
                    !(id.contains("buschain_builtins") || name == "shadow builtins")
                }
                PluginFormat::Clap | PluginFormat::Lv2 => true,
                PluginFormat::Vst3 => vst3_enabled,
            })
            .collect()
    }

    pub fn lv2_scan_count(&self) -> usize {
        self.by_format(PluginFormat::Lv2).count()
    }
}
