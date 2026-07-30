//! LADSPA `.so` loader — instantiate / connect / run (non-RT load; RT-only `run`).

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_ulong, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use libloading::Library;

use super::processor::AudioProcessor;

pub const LADSPA_PORT_INPUT: i32 = 0x1;
pub const LADSPA_PORT_OUTPUT: i32 = 0x2;
pub const LADSPA_PORT_CONTROL: i32 = 0x4;
pub const LADSPA_PORT_AUDIO: i32 = 0x8;

type LadspaHandle = *mut c_void;

#[repr(C)]
struct LadspaPortRangeHint {
    hint_descriptor: i32,
    lower_bound: f32,
    upper_bound: f32,
}

#[repr(C)]
struct LadspaDescriptor {
    unique_id: c_ulong,
    label: *const c_char,
    properties: i32,
    name: *const c_char,
    maker: *const c_char,
    copyright: *const c_char,
    port_count: c_ulong,
    port_descriptors: *const i32,
    port_names: *const *const c_char,
    port_range_hints: *const LadspaPortRangeHint,
    implementation_data: *mut c_void,
    instantiate: Option<
        unsafe extern "C" fn(descriptor: *const LadspaDescriptor, sample_rate: c_ulong) -> LadspaHandle,
    >,
    connect_port: Option<
        unsafe extern "C" fn(instance: LadspaHandle, port: c_ulong, data_location: *mut f32),
    >,
    activate: Option<unsafe extern "C" fn(instance: LadspaHandle)>,
    run: Option<unsafe extern "C" fn(instance: LadspaHandle, sample_count: c_ulong)>,
    run_adding: Option<unsafe extern "C" fn(instance: LadspaHandle, sample_count: c_ulong)>,
    set_run_adding_gain: Option<unsafe extern "C" fn(instance: LadspaHandle, gain: f32)>,
    deactivate: Option<unsafe extern "C" fn(instance: LadspaHandle)>,
    cleanup: Option<unsafe extern "C" fn(instance: LadspaHandle)>,
}

type DescriptorFn = unsafe extern "C" fn(index: c_ulong) -> *const LadspaDescriptor;

/// Keeps the shared library mapped for the lifetime of all instances from it.
pub struct LadspaLibrary {
    _lib: Library,
    descriptor_fn: DescriptorFn,
    path: PathBuf,
}

impl LadspaLibrary {
    pub fn open(path: &Path) -> Result<Arc<Self>> {
        // SAFETY: LADSPA plugins export a C ABI; we only call documented entry points.
        let lib = unsafe { Library::new(path) }
            .with_context(|| format!("dlopen {}", path.display()))?;
        let descriptor_fn: DescriptorFn = unsafe {
            *lib.get::<DescriptorFn>(b"ladspa_descriptor\0")
                .with_context(|| format!("missing ladspa_descriptor in {}", path.display()))?
        };
        Ok(Arc::new(Self {
            _lib: lib,
            descriptor_fn,
            path: path.to_path_buf(),
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn find_descriptor(&self, label: &str) -> Result<*const LadspaDescriptor> {
        let want = label.trim();
        for i in 0..256u64 {
            let d = unsafe { (self.descriptor_fn)(i as c_ulong) };
            if d.is_null() {
                break;
            }
            let lbl = unsafe { CStr::from_ptr((*d).label) }
                .to_string_lossy()
                .into_owned();
            if lbl == want {
                return Ok(d);
            }
        }
        Err(anyhow!(
            "LADSPA label `{want}` not found in {}",
            self.path.display()
        ))
    }
}

struct PortMap {
    audio_in: Vec<usize>,
    audio_out: Vec<usize>,
    control_in: Vec<usize>,
    control_names: Vec<String>,
}

fn map_ports(desc: &LadspaDescriptor) -> Result<PortMap> {
    let n = desc.port_count as usize;
    if desc.port_descriptors.is_null() || desc.port_names.is_null() {
        bail!("LADSPA descriptor missing port metadata");
    }
    let mut audio_in = Vec::new();
    let mut audio_out = Vec::new();
    let mut control_in = Vec::new();
    let mut control_names = Vec::new();
    for i in 0..n {
        let pd = unsafe { *desc.port_descriptors.add(i) };
        let name_ptr = unsafe { *desc.port_names.add(i) };
        let name = if name_ptr.is_null() {
            format!("port{i}")
        } else {
            unsafe { CStr::from_ptr(name_ptr) }
                .to_string_lossy()
                .into_owned()
        };
        if pd & LADSPA_PORT_CONTROL != 0 && pd & LADSPA_PORT_INPUT != 0 {
            control_in.push(i);
            control_names.push(name);
        } else if pd & LADSPA_PORT_AUDIO != 0 && pd & LADSPA_PORT_INPUT != 0 {
            audio_in.push(i);
        } else if pd & LADSPA_PORT_AUDIO != 0 && pd & LADSPA_PORT_OUTPUT != 0 {
            audio_out.push(i);
        }
    }
    if audio_in.is_empty() || audio_out.is_empty() {
        bail!("LADSPA plugin has no audio in/out ports");
    }
    Ok(PortMap {
        audio_in,
        audio_out,
        control_in,
        control_names,
    })
}

/// One instantiated LADSPA plugin (stereo planar host buffers).
pub struct LadspaInstance {
    _library: Arc<LadspaLibrary>,
    descriptor: *const LadspaDescriptor,
    handle: LadspaHandle,
    ports: PortMap,
    control_values: Vec<f32>,
    /// Scratch for mono plugins (sum L/R → process → duplicate).
    mono_scratch: Vec<f32>,
    /// Separate outs — many LADSPA plugins set INPLACE_BROKEN.
    out_l: Vec<f32>,
    out_r: Vec<f32>,
    sample_rate: u32,
    prepared: bool,
}

// LADSPA handles are owned exclusively by this instance; plugins we ship are RT-capable.
unsafe impl Send for LadspaInstance {}

impl LadspaInstance {
    pub fn instantiate(library: Arc<LadspaLibrary>, label: &str, sample_rate: u32) -> Result<Self> {
        let descriptor = library.find_descriptor(label)?;
        let desc = unsafe { &*descriptor };
        let ports = map_ports(desc)?;
        let instantiate = desc
            .instantiate
            .ok_or_else(|| anyhow!("plugin missing instantiate"))?;
        let handle = unsafe { instantiate(descriptor, sample_rate as c_ulong) };
        if handle.is_null() {
            bail!("LADSPA instantiate failed for `{label}`");
        }
        let n_ctrl = ports.control_in.len();
        let mut control_values = vec![0.0f32; n_ctrl];
        // Apply default hints when present.
        if !desc.port_range_hints.is_null() {
            for (ci, &port_idx) in ports.control_in.iter().enumerate() {
                let hint = unsafe { &*desc.port_range_hints.add(port_idx) };
                control_values[ci] = default_from_hint(hint);
            }
        }
        let mut inst = Self {
            _library: library,
            descriptor,
            handle,
            ports,
            control_values,
            mono_scratch: Vec::new(),
            out_l: Vec::new(),
            out_r: Vec::new(),
            sample_rate,
            prepared: false,
        };
        inst.connect_all();
        if let Some(activate) = unsafe { (*inst.descriptor).activate } {
            unsafe { activate(inst.handle) };
        }
        Ok(inst)
    }

    fn connect_all(&mut self) {
        let connect = match unsafe { (*self.descriptor).connect_port } {
            Some(f) => f,
            None => return,
        };
        for (ci, &port_idx) in self.ports.control_in.iter().enumerate() {
            unsafe {
                connect(
                    self.handle,
                    port_idx as c_ulong,
                    self.control_values.as_mut_ptr().add(ci),
                );
            }
        }
    }

    pub fn set_control_by_name(&mut self, name: &str, value: f32) -> bool {
        let Some(idx) = self
            .ports
            .control_names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
        else {
            return false;
        };
        self.set_control(idx, value);
        true
    }

    pub fn label_cstr(&self) -> Option<&CStr> {
        let p = unsafe { (*self.descriptor).label };
        if p.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(p) })
        }
    }
}

impl AudioProcessor for LadspaInstance {
    fn prepare(&mut self, sample_rate: u32, max_block: u32) {
        let n = max_block as usize;
        if self.prepared && self.sample_rate == sample_rate {
            if self.mono_scratch.len() < n {
                self.mono_scratch.resize(n, 0.0);
            }
            if self.out_l.len() < n {
                self.out_l.resize(n, 0.0);
                self.out_r.resize(n, 0.0);
            }
            return;
        }
        // Re-instantiate on rate change (non-RT path).
        if self.sample_rate != sample_rate {
            if let Some(deactivate) = unsafe { (*self.descriptor).deactivate } {
                unsafe { deactivate(self.handle) };
            }
            if let Some(cleanup) = unsafe { (*self.descriptor).cleanup } {
                unsafe { cleanup(self.handle) };
            }
            let instantiate = unsafe { (*self.descriptor).instantiate }
                .expect("instantiate");
            let handle = unsafe { instantiate(self.descriptor, sample_rate as c_ulong) };
            self.handle = handle;
            self.sample_rate = sample_rate;
            self.connect_all();
            if let Some(activate) = unsafe { (*self.descriptor).activate } {
                unsafe { activate(self.handle) };
            }
        }
        self.mono_scratch.resize(n, 0.0);
        self.out_l.resize(n, 0.0);
        self.out_r.resize(n, 0.0);
        self.prepared = true;
    }

    fn latency_samples(&self) -> u32 {
        0
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        debug_assert_eq!(left.len(), right.len());
        let n = left.len();
        if n == 0 {
            return;
        }
        let run = match unsafe { (*self.descriptor).run } {
            Some(f) => f,
            None => return,
        };
        let connect = match unsafe { (*self.descriptor).connect_port } {
            Some(f) => f,
            None => return,
        };

        let n_in = self.ports.audio_in.len();
        let n_out = self.ports.audio_out.len();
        let use_n = n.min(self.out_l.len()).min(self.out_r.len());
        if use_n == 0 {
            return;
        }

        if n_in >= 2 && n_out >= 2 {
            unsafe {
                connect(
                    self.handle,
                    self.ports.audio_in[0] as c_ulong,
                    left.as_mut_ptr(),
                );
                connect(
                    self.handle,
                    self.ports.audio_in[1] as c_ulong,
                    right.as_mut_ptr(),
                );
                connect(
                    self.handle,
                    self.ports.audio_out[0] as c_ulong,
                    self.out_l.as_mut_ptr(),
                );
                connect(
                    self.handle,
                    self.ports.audio_out[1] as c_ulong,
                    self.out_r.as_mut_ptr(),
                );
                run(self.handle, use_n as c_ulong);
            }
            left[..use_n].copy_from_slice(&self.out_l[..use_n]);
            right[..use_n].copy_from_slice(&self.out_r[..use_n]);
        } else {
            // Mono: average → process into out_l → duplicate.
            let mono_n = use_n.min(self.mono_scratch.len());
            if mono_n == 0 {
                return;
            }
            for i in 0..mono_n {
                self.mono_scratch[i] = 0.5 * (left[i] + right[i]);
            }
            unsafe {
                connect(
                    self.handle,
                    self.ports.audio_in[0] as c_ulong,
                    self.mono_scratch.as_mut_ptr(),
                );
                connect(
                    self.handle,
                    self.ports.audio_out[0] as c_ulong,
                    self.out_l.as_mut_ptr(),
                );
                run(self.handle, mono_n as c_ulong);
            }
            for i in 0..mono_n {
                let s = self.out_l[i];
                left[i] = s;
                right[i] = s;
            }
        }
    }

    fn set_control(&mut self, index: usize, value: f32) {
        if let Some(slot) = self.control_values.get_mut(index) {
            *slot = value;
        }
    }

    fn control_count(&self) -> usize {
        self.control_values.len()
    }

    fn control_name(&self, index: usize) -> Option<&str> {
        self.ports.control_names.get(index).map(|s| s.as_str())
    }
}

impl Drop for LadspaInstance {
    fn drop(&mut self) {
        if self.handle.is_null() {
            return;
        }
        unsafe {
            if let Some(deactivate) = (*self.descriptor).deactivate {
                deactivate(self.handle);
            }
            if let Some(cleanup) = (*self.descriptor).cleanup {
                cleanup(self.handle);
            }
        }
        self.handle = std::ptr::null_mut();
    }
}

fn default_from_hint(hint: &LadspaPortRangeHint) -> f32 {
    const DEFAULT_MASK: i32 = 0x3C0;
    let d = hint.hint_descriptor & DEFAULT_MASK;
    match d {
        0x40 => hint.lower_bound,  // MINIMUM
        0x80 => hint.lower_bound + (hint.upper_bound - hint.lower_bound) * 0.25, // LOW
        0xC0 => 0.5 * (hint.lower_bound + hint.upper_bound), // MIDDLE
        0x100 => hint.lower_bound + (hint.upper_bound - hint.lower_bound) * 0.75, // HIGH
        0x140 => hint.upper_bound, // MAXIMUM
        0x200 => 0.0,
        0x240 => 1.0,
        0x280 => 100.0,
        0x2C0 => 440.0,
        _ => {
            if hint.hint_descriptor & 0x1 != 0 {
                hint.lower_bound
            } else {
                0.0
            }
        }
    }
}

/// Resolve `.so` path for a plugin stem or absolute path.
pub fn resolve_plugin_so(plugin_so: &str, search_path: &str) -> Option<PathBuf> {
    let p = Path::new(plugin_so);
    if p.is_absolute() && p.exists() {
        return Some(p.to_path_buf());
    }
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(plugin_so)
        .trim_end_matches(".so");
    let candidates = [
        format!("{stem}.so"),
        format!("lib{stem}.so"),
        format!("{stem}_ladspa.so"),
    ];
    for dir in search_path.split(':').filter(|s| !s.is_empty()) {
        for c in &candidates {
            let full = Path::new(dir).join(c);
            if full.exists() {
                return Some(full);
            }
        }
    }
    None
}

/// Load instance from InsertSlot fields.
pub fn load_instance(
    plugin_so: &str,
    label: &str,
    sample_rate: u32,
    search_path: &str,
) -> Result<LadspaInstance> {
    let path = resolve_plugin_so(plugin_so, search_path)
        .or_else(|| resolve_plugin_so(label, search_path))
        .ok_or_else(|| anyhow!("cannot resolve LADSPA .so for `{plugin_so}` / `{label}`"))?;
    let lib = LadspaLibrary::open(&path)?;
    let label = label.trim();
    let label = label.strip_prefix("ladspa:").unwrap_or(label);
    LadspaInstance::instantiate(lib, label, sample_rate)
}

/// Keep unused CString helper available for future port-name FFI.
#[allow(dead_code)]
fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|e| anyhow!("{e}"))
}
