//! PipeWire filter node — process() runs inside the PW RT data callback.

use std::ffi::{c_void, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, bail, Result};
use once_cell::sync::OnceCell;
use pipewire as pw;
use pw::sys as pw_sys;

use super::control::{ControlMsg, ControlQueue};
use super::denormal::DenormalGuard;
use super::processor::MidiEvent;
use super::rack::Rack;
use crate::midi::MidiEventQueue;

static PW_INIT: OnceCell<()> = OnceCell::new();

fn ensure_pw_init() {
    PW_INIT.get_or_init(|| {
        pw::init();
    });
}

/// Shared RT state — atomic Arc swap for the rack (no mutex in the audio path).
pub struct HostRtState {
    pub queue: ControlQueue,
    /// M2: optional MIDI events for `AudioProcessor::feed_midi`.
    pub midi_queue: MidiEventQueue,
    /// `Arc<Mutex<Rack>>` as raw pointer; null = empty.
    rack_ptr: AtomicPtr<Mutex<Rack>>,
    pub published_gen: AtomicU64,
    pub observed_gen: AtomicU64,
    pub latency_samples: AtomicU32,
    pub xruns: AtomicU64,
    pub process_calls: AtomicU64,
    pub running: AtomicBool,
    pub streaming: AtomicBool,
    pub quit: AtomicBool,
    pub filter: AtomicPtr<pw_sys::pw_filter>,
    pub node_id: AtomicU32,
    /// Peak |sample| as f32 bits (pre-insert / post-insert).
    pub meter_pre_peak: AtomicU32,
    pub meter_post_peak: AtomicU32,
    /// Master-bus PDC delay line (try_lock in RT; lock on worker resize).
    pub pdc: Mutex<super::pdc::DelayLine>,
}

impl HostRtState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: ControlQueue::new(),
            midi_queue: MidiEventQueue::new(),
            rack_ptr: AtomicPtr::new(ptr::null_mut()),
            published_gen: AtomicU64::new(0),
            observed_gen: AtomicU64::new(0),
            latency_samples: AtomicU32::new(0),
            xruns: AtomicU64::new(0),
            process_calls: AtomicU64::new(0),
            running: AtomicBool::new(false),
            streaming: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            filter: AtomicPtr::new(ptr::null_mut()),
            node_id: AtomicU32::new(0),
            meter_pre_peak: AtomicU32::new(0),
            meter_post_peak: AtomicU32::new(0),
            pdc: Mutex::new(super::pdc::DelayLine::new()),
        })
    }

    pub fn publish_rack(&self, rack: Rack) {
        let gen = rack.generation;
        let lat = rack.total_latency_samples();
        let new = Arc::new(Mutex::new(rack));
        let new_ptr = Arc::into_raw(new) as *mut Mutex<Rack>;
        let old = self.rack_ptr.swap(new_ptr, Ordering::AcqRel);
        self.latency_samples.store(lat, Ordering::Release);
        self.published_gen.store(gen, Ordering::Release);
        if !old.is_null() {
            // Drop previous generation on the publisher (non-RT) thread.
            unsafe {
                drop(Arc::from_raw(old));
            }
        }
    }

    /// Non-RT: resize PDC delay for Master-bus align.
    pub fn set_pdc_delay(&self, samples: u32) {
        if let Ok(mut d) = self.pdc.lock() {
            d.set_delay(samples);
        }
    }

    pub fn meter_peaks(&self) -> (f32, f32) {
        (
            f32::from_bits(self.meter_pre_peak.load(Ordering::Relaxed)),
            f32::from_bits(self.meter_post_peak.load(Ordering::Relaxed)),
        )
    }

    /// Non-RT: borrow current rack Arc (strong clone).
    pub fn load_rack_arc(&self) -> Option<Arc<Mutex<Rack>>> {
        let ptr = self.rack_ptr.load(Ordering::Acquire);
        if ptr.is_null() {
            return None;
        }
        let arc = unsafe { Arc::from_raw(ptr) };
        let out = Arc::clone(&arc);
        std::mem::forget(arc);
        Some(out)
    }
}

impl Drop for HostRtState {
    fn drop(&mut self) {
        let ptr = self.rack_ptr.swap(ptr::null_mut(), Ordering::AcqRel);
        if !ptr.is_null() {
            unsafe {
                drop(Arc::from_raw(ptr));
            }
        }
    }
}

struct FilterPorts {
    in_l: *mut c_void,
    in_r: *mut c_void,
    out_l: *mut c_void,
    out_r: *mut c_void,
}

struct FilterUserData {
    state: Arc<HostRtState>,
    ports: FilterPorts,
    drain: [ControlMsg; 256],
    midi_drain: [MidiEvent; 64],
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
    max_block: u32,
}

#[derive(Clone, Copy)]
struct MainLoopPtr(*mut pw_sys::pw_main_loop);
unsafe impl Send for MainLoopPtr {}

/// One in-process FX filter for a bus (`node.name = buschain_fx_{bus}`).
pub struct PwFxNode {
    pub bus: String,
    pub fx_name: String,
    pub state: Arc<HostRtState>,
    thread: Option<JoinHandle<()>>,
    loop_ptr: Arc<Mutex<Option<MainLoopPtr>>>,
}

unsafe impl Send for PwFxNode {}

impl PwFxNode {
    pub fn start(bus: &str, sample_rate: u32, quantum: u32) -> Result<Self> {
        ensure_pw_init();
        let fx_name = crate::domain::fx_name_for_bus(bus);
        let state = HostRtState::new();
        let loop_ptr: Arc<Mutex<Option<MainLoopPtr>>> = Arc::new(Mutex::new(None));
        let state_t = state.clone();
        let loop_t = loop_ptr.clone();
        let name = fx_name.clone();

        let thread = thread::Builder::new()
            .name(format!("pw-fx-{bus}"))
            .spawn(move || {
                if let Err(e) = run_filter_loop(&name, state_t, loop_t, sample_rate, quantum) {
                    eprintln!("[buschain host] filter `{name}` exited: {e:#}");
                }
            })
            .map_err(|e| anyhow!("spawn filter thread: {e}"))?;

        // Wait only on the in-process `running` flag set after pw_filter_connect.
        // Never poll pw-cli / pw-link here — that was multi-second per bus.
        let t0 = std::time::Instant::now();
        while t0.elapsed() < std::time::Duration::from_millis(150) {
            if state.running.load(Ordering::Acquire) {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(2));
        }
        // One cache invalidate so the first ensure_link sees new ports.
        crate::backend::invalidate_probe_caches();

        Ok(Self {
            bus: bus.to_string(),
            fx_name,
            state,
            thread: Some(thread),
            loop_ptr,
        })
    }

    pub fn is_running(&self) -> bool {
        self.state.running.load(Ordering::Acquire)
    }

    pub fn publish_rack(&self, rack: Rack) {
        self.state.publish_rack(rack);
    }

    pub fn stop(mut self) {
        self.shutdown();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn shutdown(&mut self) {
        self.state.quit.store(true, Ordering::Release);
        if let Ok(g) = self.loop_ptr.lock() {
            if let Some(MainLoopPtr(lp)) = *g {
                unsafe {
                    pw_sys::pw_main_loop_quit(lp);
                }
            }
        }
    }
}

impl Drop for PwFxNode {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run_filter_loop(
    fx_name: &str,
    state: Arc<HostRtState>,
    loop_slot: Arc<Mutex<Option<MainLoopPtr>>>,
    sample_rate: u32,
    quantum: u32,
) -> Result<()> {
    unsafe {
        let loop_ = pw_sys::pw_main_loop_new(ptr::null());
        if loop_.is_null() {
            bail!("pw_main_loop_new failed");
        }
        *loop_slot.lock().unwrap() = Some(MainLoopPtr(loop_));

        let name_c = CString::new(fx_name)?;
        let desc_c = CString::new(format!("BusChain FX {fx_name}"))?;
        let rate_c = CString::new(sample_rate.to_string())?;
        let q_c = CString::new(quantum.max(64).to_string())?;

        // Do NOT force audio.rate — follow the graph driver (buses may be 48k/96k).
        // always-process keeps the node scheduled once linked into a running graph.
        let _ = (rate_c, sample_rate);
        let props = pw_sys::pw_properties_new(
            c"media.type".as_ptr(),
            c"Audio".as_ptr(),
            c"media.category".as_ptr(),
            c"Filter".as_ptr(),
            c"media.role".as_ptr(),
            c"DSP".as_ptr(),
            c"node.name".as_ptr(),
            name_c.as_ptr(),
            c"node.description".as_ptr(),
            desc_c.as_ptr(),
            c"media.class".as_ptr(),
            c"Audio/Duplex".as_ptr(),
            c"node.virtual".as_ptr(),
            c"true".as_ptr(),
            c"node.passive".as_ptr(),
            c"false".as_ptr(),
            c"node.want-driver".as_ptr(),
            c"true".as_ptr(),
            c"node.always-process".as_ptr(),
            c"true".as_ptr(),
            c"clock.quantum-limit".as_ptr(),
            q_c.as_ptr(),
            ptr::null::<i8>(),
        );

        let mut userdata = Box::new(FilterUserData {
            state: state.clone(),
            ports: FilterPorts {
                in_l: ptr::null_mut(),
                in_r: ptr::null_mut(),
                out_l: ptr::null_mut(),
                out_r: ptr::null_mut(),
            },
            drain: [ControlMsg::bypass(uuid::Uuid::nil(), false); 256],
            midi_drain: [MidiEvent::Cc {
                channel: 0,
                controller: 0,
                value: 0,
            }; 64],
            scratch_l: vec![0.0f32; quantum.max(2048) as usize],
            scratch_r: vec![0.0f32; quantum.max(2048) as usize],
            max_block: quantum.max(2048),
        });

        let events = pw_sys::pw_filter_events {
            version: pw_sys::PW_VERSION_FILTER_EVENTS,
            destroy: None,
            state_changed: Some(on_state_changed),
            io_changed: None,
            param_changed: None,
            add_buffer: None,
            remove_buffer: None,
            process: Some(on_process),
            drained: None,
            command: None,
        };

        let filter = pw_sys::pw_filter_new_simple(
            pw_sys::pw_main_loop_get_loop(loop_),
            name_c.as_ptr(),
            props,
            &events,
            userdata.as_mut() as *mut FilterUserData as *mut c_void,
        );
        if filter.is_null() {
            pw_sys::pw_main_loop_destroy(loop_);
            bail!("pw_filter_new_simple failed");
        }
        state.filter.store(filter, Ordering::Release);

        userdata.ports.in_l =
            add_dsp_port(filter, spa_sys::SPA_DIRECTION_INPUT, "input_FL")?;
        userdata.ports.in_r =
            add_dsp_port(filter, spa_sys::SPA_DIRECTION_INPUT, "input_FR")?;
        userdata.ports.out_l =
            add_dsp_port(filter, spa_sys::SPA_DIRECTION_OUTPUT, "output_FL")?;
        userdata.ports.out_r =
            add_dsp_port(filter, spa_sys::SPA_DIRECTION_OUTPUT, "output_FR")?;

        let rc = pw_sys::pw_filter_connect(
            filter,
            pw_sys::pw_filter_flags_PW_FILTER_FLAG_RT_PROCESS,
            ptr::null_mut(),
            0,
        );
        if rc < 0 {
            state.filter.store(ptr::null_mut(), Ordering::Release);
            pw_sys::pw_filter_destroy(filter);
            pw_sys::pw_main_loop_destroy(loop_);
            bail!("pw_filter_connect failed ({rc})");
        }

        let nid = pw_sys::pw_filter_get_node_id(filter);
        // SPA_ID_INVALID is typically !0; 0 means not assigned yet.
        if nid != 0 && nid != u32::MAX {
            state.node_id.store(nid, Ordering::Release);
            crate::backend::cache_node_id(fx_name, nid);
        }
        // Connected — ready for links. Do not wait for STREAMING (needs graph links).
        state.running.store(true, Ordering::Release);
        pw_sys::pw_main_loop_run(loop_);

        state.streaming.store(false, Ordering::Release);
        state.running.store(false, Ordering::Release);
        state.filter.store(ptr::null_mut(), Ordering::Release);
        pw_sys::pw_filter_destroy(filter);
        {
            let mut slot = loop_slot.lock().unwrap();
            *slot = None;
        }
        pw_sys::pw_main_loop_destroy(loop_);
        drop(userdata);
    }
    Ok(())
}

unsafe fn add_dsp_port(
    filter: *mut pw_sys::pw_filter,
    direction: spa_sys::spa_direction,
    name: &str,
) -> Result<*mut c_void> {
    let name_c = CString::new(name)?;
    let props = pw_sys::pw_properties_new(
        c"format.dsp".as_ptr(),
        c"32 bit float mono audio".as_ptr(),
        c"port.name".as_ptr(),
        name_c.as_ptr(),
        ptr::null::<i8>(),
    );
    let port = pw_sys::pw_filter_add_port(
        filter,
        direction,
        pw_sys::pw_filter_port_flags_PW_FILTER_PORT_FLAG_MAP_BUFFERS,
        0,
        props,
        ptr::null_mut(),
        0,
    );
    if port.is_null() {
        bail!("pw_filter_add_port `{name}` failed");
    }
    Ok(port)
}

unsafe extern "C" fn on_state_changed(
    data: *mut c_void,
    _old: pw_sys::pw_filter_state,
    state: pw_sys::pw_filter_state,
    _error: *const i8,
) {
    if data.is_null() {
        return;
    }
    let ud = &*(data as *const FilterUserData);
    match state {
        s if s == pw_sys::pw_filter_state_PW_FILTER_STATE_STREAMING => {
            ud.state.running.store(true, Ordering::Release);
            ud.state.streaming.store(true, Ordering::Release);
        }
        s if s == pw_sys::pw_filter_state_PW_FILTER_STATE_PAUSED => {
            ud.state.running.store(true, Ordering::Release);
            ud.state.streaming.store(false, Ordering::Release);
        }
        s if s == pw_sys::pw_filter_state_PW_FILTER_STATE_ERROR => {
            ud.state.streaming.store(false, Ordering::Release);
            ud.state.xruns.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
}

unsafe extern "C" fn on_process(data: *mut c_void, position: *mut spa_sys::spa_io_position) {
    if data.is_null() || position.is_null() {
        return;
    }
    let ud = &mut *(data as *mut FilterUserData);
    let n = (*position).clock.duration as u32;
    if n == 0 {
        return;
    }
    let n = n.min(ud.max_block);
    let ns = n as usize;

    let in_l = pw_sys::pw_filter_get_dsp_buffer(ud.ports.in_l, n) as *mut f32;
    let in_r = pw_sys::pw_filter_get_dsp_buffer(ud.ports.in_r, n) as *mut f32;
    let out_l = pw_sys::pw_filter_get_dsp_buffer(ud.ports.out_l, n) as *mut f32;
    let out_r = pw_sys::pw_filter_get_dsp_buffer(ud.ports.out_r, n) as *mut f32;

    if out_l.is_null() || out_r.is_null() {
        // Normal before graph links / while paused — not an xrun.
        return;
    }

    if !in_l.is_null() && !in_r.is_null() {
        ptr::copy_nonoverlapping(in_l, ud.scratch_l.as_mut_ptr(), ns);
        ptr::copy_nonoverlapping(in_r, ud.scratch_r.as_mut_ptr(), ns);
    } else if !in_l.is_null() {
        ptr::copy_nonoverlapping(in_l, ud.scratch_l.as_mut_ptr(), ns);
        ptr::copy_nonoverlapping(in_l, ud.scratch_r.as_mut_ptr(), ns);
    } else {
        for i in 0..ns {
            ud.scratch_l[i] = 0.0;
            ud.scratch_r[i] = 0.0;
        }
    }

    let drain_n = ud.state.queue.drain_to(&mut ud.drain);
    let midi_n = ud.state.midi_queue.drain_to(&mut ud.midi_drain);

    // Pre-insert meter peak (atomic f32 bits).
    let mut pre_peak = 0.0f32;
    for i in 0..ns {
        pre_peak = pre_peak.max(ud.scratch_l[i].abs()).max(ud.scratch_r[i].abs());
    }
    ud.state
        .meter_pre_peak
        .store(pre_peak.to_bits(), Ordering::Relaxed);

    // Wait-free load of current rack Arc (no mutex on the publish slot).
    let ptr = ud.state.rack_ptr.load(Ordering::Acquire);
    if !ptr.is_null() {
        let arc = unsafe { Arc::from_raw(ptr) };
        let rack_arc = Arc::clone(&arc);
        std::mem::forget(arc);

        let _denorm = DenormalGuard::enter();
        let locked = rack_arc.try_lock();
        if let Ok(mut rack) = locked {
            if drain_n > 0 {
                rack.apply_controls(&ud.drain[..drain_n]);
            }
            if midi_n > 0 {
                rack.feed_midi(&ud.midi_drain[..midi_n]);
            }
            rack.process(&mut ud.scratch_l[..ns], &mut ud.scratch_r[..ns]);
            ud.state
                .observed_gen
                .store(rack.generation, Ordering::Release);
        } else if ud.state.streaming.load(Ordering::Relaxed) {
            // Contended only while non-RT rebuild holds the mutex briefly.
            ud.state.xruns.fetch_add(1, Ordering::Relaxed);
        }
    }

    // Master-bus PDC (try_lock — skip if worker is resizing).
    if let Ok(mut d) = ud.state.pdc.try_lock() {
        d.process(&mut ud.scratch_l[..ns], &mut ud.scratch_r[..ns]);
    }

    let mut post_peak = 0.0f32;
    for i in 0..ns {
        post_peak = post_peak
            .max(ud.scratch_l[i].abs())
            .max(ud.scratch_r[i].abs());
    }
    ud.state
        .meter_post_peak
        .store(post_peak.to_bits(), Ordering::Relaxed);

    ptr::copy_nonoverlapping(ud.scratch_l.as_ptr(), out_l, ns);
    ptr::copy_nonoverlapping(ud.scratch_r.as_ptr(), out_r, ns);
    ud.state.process_calls.fetch_add(1, Ordering::Relaxed);
}
