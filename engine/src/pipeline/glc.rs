//! Master fan-in graph latency compensation (GLC).
//!
//! Contract: for each track stem `T` that feeds `buschain_master`,
//!   L(T) = L_local(T) + max { L(U) : U → T via track egress }
//!   L*   = max { L(T) : T → Master }
//!   δ(T) = L* − L(T)   (Master-edge pad only; Track→Track stays undelayed)
//!
//! Cycles in track egress are rejected (DAG required). Control plane is O(N+E).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{bail, Result};
use once_cell::sync::Lazy;

use crate::clock::GraphClock;
use crate::domain::glc_name_for_bus as domain_glc_name;
use crate::host::node::{HostRtState, PwFxNode};
use crate::host::{self as host_api};
use crate::plan::DesiredState;

/// Fixed BusChain stage cost in graph quanta: null-sink bus + post.
/// In-process FX is not counted as a full period (same-cycle filter).
const STAGE_QUANTA: u32 = 2;

/// GLC filter node itself contributes ~1 quantum of graph buffering.
const GLC_NODE_QUANTA: u32 = 1;

/// Result of a GLC recompute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlcPlan {
    /// Path latency to each track's wet out.
    pub path_latency: HashMap<String, u32>,
    /// Master fan-in max path latency.
    pub l_star: u32,
    /// Master-edge pad δ(T) for tracks that feed Master.
    pub master_pad: HashMap<String, u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlcError {
    /// Audio egress cycle involving these buses (in discovery order).
    Cycle(Vec<String>),
}

impl std::fmt::Display for GlcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GlcError::Cycle(nodes) => write!(f, "track egress cycle: {}", nodes.join(" → ")),
        }
    }
}

impl std::error::Error for GlcError {}

/// `buschain_glc_<suffix>` — Internal Master-edge delay helper.
pub fn glc_name_for_bus(bus: &str) -> String {
    domain_glc_name(bus)
}

pub fn is_glc_node(name: &str) -> bool {
    name.starts_with("buschain_glc_")
}

fn pads() -> &'static Mutex<HashMap<String, AtomicU32>> {
    static P: OnceLock<Mutex<HashMap<String, AtomicU32>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

fn paths() -> &'static Mutex<HashMap<String, AtomicU32>> {
    static P: OnceLock<Mutex<HashMap<String, AtomicU32>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

fn l_star_atom() -> &'static AtomicU32 {
    static L: AtomicU32 = AtomicU32::new(0);
    &L
}

fn last_quantum() -> &'static AtomicU32 {
    static Q: AtomicU32 = AtomicU32::new(256);
    &Q
}

fn disabled_flag() -> &'static AtomicU32 {
    // 0 = GLC on, 1 = Master Direct (disabled).
    static D: AtomicU32 = AtomicU32::new(1);
    &D
}

/// True when Master Direct Out is on — no Master-edge GLC nodes.
pub fn is_disabled() -> bool {
    disabled_flag().load(Ordering::Acquire) != 0
}

fn set_disabled(disabled: bool) {
    disabled_flag().store(u32::from(disabled), Ordering::Release);
}

/// Last computed Master pad δ for `bus` (0 = direct link, no GLC node).
pub fn master_pad_samples(bus: &str) -> u32 {
    pads()
        .lock()
        .ok()
        .and_then(|g| g.get(bus).map(|a| a.load(Ordering::Acquire)))
        .unwrap_or(0)
}

/// Path latency L(T) to this stem's wet out (samples).
pub fn path_latency_samples(bus: &str) -> u32 {
    paths()
        .lock()
        .ok()
        .and_then(|g| g.get(bus).map(|a| a.load(Ordering::Acquire)))
        .unwrap_or(0)
}

pub fn l_star_samples() -> u32 {
    l_star_atom().load(Ordering::Acquire)
}

/// DelayLine length for a published δ — subtracts the GLC filter's own ~1 quantum
/// so the node hop is not double-counted on top of δ.
pub fn delay_line_samples(pad: u32, quantum: u32) -> u32 {
    if pad == 0 {
        return 0;
    }
    pad.saturating_sub(GLC_NODE_QUANTA.saturating_mul(quantum.max(1)))
}

/// Publish a computed plan into the process-wide pad / path maps.
pub fn publish_plan(plan: &GlcPlan) {
    l_star_atom().store(plan.l_star, Ordering::Release);
    if let Ok(mut g) = pads().lock() {
        let keep: HashSet<&str> = plan.master_pad.keys().map(|s| s.as_str()).collect();
        g.retain(|k, _| keep.contains(k.as_str()));
        for (bus, d) in &plan.master_pad {
            g.entry(bus.clone())
                .or_insert_with(|| AtomicU32::new(0))
                .store(*d, Ordering::Release);
        }
    }
    if let Ok(mut g) = paths().lock() {
        let keep: HashSet<&str> = plan.path_latency.keys().map(|s| s.as_str()).collect();
        g.retain(|k, _| keep.contains(k.as_str()));
        for (bus, l) in &plan.path_latency {
            g.entry(bus.clone())
                .or_insert_with(|| AtomicU32::new(0))
                .store(*l, Ordering::Release);
        }
    }
}

/// Local stem cost: reported rack latency + fixed stage quanta × quantum.
pub fn local_latency_samples(bus: &str, quantum: u32) -> u32 {
    let rack = host_api::reported_latency(bus);
    let q = quantum.max(1);
    rack.saturating_add(STAGE_QUANTA.saturating_mul(q))
}

/// Pure compute from an adjacency list and per-bus local costs.
///
/// `feeds_master` lists buses that have Master in their egress.
/// `edges` are directed track→track feeds (U → T means U's wet out enters T).
pub fn compute_glc(
    locals: &HashMap<String, u32>,
    edges: &[(String, String)],
    feeds_master: &HashSet<String>,
) -> Result<GlcPlan, GlcError> {
    let mut nodes: HashSet<String> = locals.keys().cloned().collect();
    for (a, b) in edges {
        nodes.insert(a.clone());
        nodes.insert(b.clone());
    }
    for b in feeds_master {
        nodes.insert(b.clone());
    }

    // Build adjacency + indegree for Kahn topo; also reverse edges for DP.
    let mut succ: HashMap<String, Vec<String>> = HashMap::new();
    let mut pred: HashMap<String, Vec<String>> = HashMap::new();
    let mut indeg: HashMap<String, u32> = nodes.iter().map(|n| (n.clone(), 0)).collect();
    for (u, v) in edges {
        if u == v {
            return Err(GlcError::Cycle(vec![u.clone(), v.clone()]));
        }
        succ.entry(u.clone()).or_default().push(v.clone());
        pred.entry(v.clone()).or_default().push(u.clone());
        *indeg.entry(v.clone()).or_insert(0) += 1;
        indeg.entry(u.clone()).or_insert(0);
    }

    let mut q: VecDeque<String> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(n, _)| n.clone())
        .collect();
    let mut topo: Vec<String> = Vec::with_capacity(nodes.len());
    let mut seen = 0u32;
    let mut indeg_work = indeg.clone();
    while let Some(n) = q.pop_front() {
        topo.push(n.clone());
        seen += 1;
        if let Some(vs) = succ.get(&n) {
            for v in vs {
                if let Some(d) = indeg_work.get_mut(v) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        q.push_back(v.clone());
                    }
                }
            }
        }
    }
    if seen as usize != nodes.len() {
        // Reconstruct a simple cycle witness from remaining indegree > 0.
        let mut cycle: Vec<String> = indeg_work
            .iter()
            .filter(|(_, d)| **d > 0)
            .map(|(n, _)| n.clone())
            .collect();
        cycle.sort();
        return Err(GlcError::Cycle(cycle));
    }

    let mut path_latency: HashMap<String, u32> = HashMap::new();
    for n in &topo {
        let local = locals.get(n).copied().unwrap_or(0);
        let upstream = pred
            .get(n)
            .map(|ps| {
                ps.iter()
                    .map(|p| path_latency.get(p).copied().unwrap_or(0))
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        path_latency.insert(n.clone(), local.saturating_add(upstream));
    }

    let l_star = feeds_master
        .iter()
        .map(|b| path_latency.get(b).copied().unwrap_or(0))
        .max()
        .unwrap_or(0);

    let mut master_pad = HashMap::new();
    for b in feeds_master {
        let l = path_latency.get(b).copied().unwrap_or(0);
        master_pad.insert(b.clone(), l_star.saturating_sub(l));
    }

    Ok(GlcPlan {
        path_latency,
        l_star,
        master_pad,
    })
}

#[derive(Clone, Default)]
struct TopoCache {
    quantum: u32,
    buses: Vec<String>,
    edges: Vec<(String, String)>,
    feeds_master: HashSet<String>,
}

fn topo_cache() -> &'static Mutex<TopoCache> {
    static T: OnceLock<Mutex<TopoCache>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(TopoCache::default()))
}

fn extract_topo(desired: &DesiredState) -> (HashMap<String, u32>, TopoCache) {
    let quantum = desired.clock.quantum.max(1);
    let direct = &desired.glc_direct;
    let mut locals: HashMap<String, u32> = HashMap::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut feeds_master: HashSet<String> = HashSet::new();
    let mut buses: Vec<String> = Vec::new();

    for (bus, spec) in &desired.buses {
        if !matches!(
            spec.role,
            crate::domain::NodeRole::TrackBus | crate::domain::NodeRole::MasterBus
        ) {
            continue;
        }
        if bus == "buschain_master" || direct.contains(bus) {
            continue;
        }
        buses.push(bus.clone());
        locals.insert(bus.clone(), local_latency_samples(bus, quantum));
    }

    for (bus, dests) in &desired.bus_egress {
        if bus == "buschain_master" || bus.starts_with("buschain_vinf_") {
            continue;
        }
        // Direct Out buses are outside the sync contract.
        if direct.contains(bus) {
            continue;
        }
        if !locals.contains_key(bus) {
            buses.push(bus.clone());
            locals.insert(bus.clone(), local_latency_samples(bus, quantum));
        }
        for d in dests {
            if d.is_empty() {
                continue;
            }
            if d == "buschain_master" {
                feeds_master.insert(bus.clone());
            } else if d.starts_with("buschain_track_") {
                // Edges into Direct destinations are ignored (sink is out of contract).
                if direct.contains(d) {
                    continue;
                }
                if !locals.contains_key(d) {
                    buses.push(d.clone());
                    locals.insert(d.clone(), local_latency_samples(d, quantum));
                }
                edges.push((bus.clone(), d.clone()));
            }
        }
    }

    let cache = TopoCache {
        quantum,
        buses,
        edges,
        feeds_master,
    };
    (locals, cache)
}

fn empty_plan() -> GlcPlan {
    GlcPlan {
        path_latency: HashMap::new(),
        l_star: 0,
        master_pad: HashMap::new(),
    }
}

/// Build edge list + Master feeders from Desired egress, then compute + publish.
pub fn recompute_from_desired(desired: &DesiredState) -> Result<GlcPlan, GlcError> {
    set_disabled(desired.glc_disabled);
    last_quantum().store(desired.clock.quantum.max(1), Ordering::Release);
    if desired.glc_disabled {
        let plan = empty_plan();
        if let Ok(mut g) = topo_cache().lock() {
            *g = TopoCache::default();
        }
        publish_plan(&plan);
        teardown_all_nodes();
        return Ok(plan);
    }

    let (locals, cache) = extract_topo(desired);
    let plan = compute_glc(&locals, &cache.edges, &cache.feeds_master)?;
    // Force Direct buses to δ=0 even if they somehow remain in Desired feeds.
    let mut plan = plan;
    for bus in &desired.glc_direct {
        plan.master_pad.insert(bus.clone(), 0);
    }
    last_quantum().store(cache.quantum.max(1), Ordering::Release);
    if let Ok(mut g) = topo_cache().lock() {
        *g = cache;
    }
    publish_plan(&plan);
    Ok(plan)
}

/// Recompute pads from the last Desired topology (latency publish path).
pub fn recompute_from_cached_topo() -> Result<GlcPlan, GlcError> {
    if is_disabled() {
        let plan = empty_plan();
        publish_plan(&plan);
        teardown_all_nodes();
        return Ok(plan);
    }
    let cache = topo_cache().lock().ok().map(|g| g.clone()).unwrap_or_default();
    if cache.feeds_master.is_empty() && cache.edges.is_empty() {
        let plan = empty_plan();
        publish_plan(&plan);
        return Ok(plan);
    }
    let mut locals = HashMap::new();
    for bus in &cache.buses {
        locals.insert(bus.clone(), local_latency_samples(bus, cache.quantum));
    }
    for (a, b) in &cache.edges {
        locals
            .entry(a.clone())
            .or_insert_with(|| local_latency_samples(a, cache.quantum));
        locals
            .entry(b.clone())
            .or_insert_with(|| local_latency_samples(b, cache.quantum));
    }
    last_quantum().store(cache.quantum.max(1), Ordering::Release);
    let plan = compute_glc(&locals, &cache.edges, &cache.feeds_master)?;
    publish_plan(&plan);
    Ok(plan)
}

/// Fallible wrapper for runtime (cycles become anyhow errors).
pub fn recompute_from_desired_report(desired: &DesiredState) -> Result<GlcPlan> {
    match recompute_from_desired(desired) {
        Ok(p) => Ok(p),
        Err(e) => bail!("{e}"),
    }
}

/// Map Desired egress dests to physical link targets (Master → GLC when δ>0).
pub fn physical_egress_dests(bus: &str, dests: &[String]) -> Vec<String> {
    dests
        .iter()
        .map(|d| {
            if d == "buschain_master"
                && !is_disabled()
                && master_pad_samples(bus) > 0
            {
                glc_name_for_bus(bus)
            } else {
                d.clone()
            }
        })
        .collect()
}

/// True when `δ=0` for Master (direct post→Master; no `buschain_glc_*` node).
pub fn master_uses_direct_link(bus: &str) -> bool {
    master_pad_samples(bus) == 0
}

// ── Live GLC filter nodes (Master-edge DelayLine) ───────────────────────────

static GLC_NODES: Lazy<Mutex<HashMap<String, GlcNode>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static GLC_STATE: Lazy<Mutex<HashMap<String, Arc<HostRtState>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

struct GlcNode {
    node: PwFxNode,
}

fn clock_sr(clock: &GraphClock) -> u32 {
    clock.sample_rate.max(8_000)
}

fn clock_q(clock: &GraphClock) -> u32 {
    clock.quantum.max(64)
}

/// Ensure / teardown Master-edge GLC nodes from a published plan + clock.
///
/// `δ=0` ⇒ no node (direct Master link). `δ>0` ⇒ `buschain_glc_*` with DelayLine(δ)
/// and `glc → buschain_master` linked.
pub fn sync_nodes(plan: &GlcPlan, clock: &GraphClock) -> Result<()> {
    if is_disabled() {
        teardown_all_nodes();
        return Ok(());
    }
    let want: HashSet<String> = plan
        .master_pad
        .iter()
        .filter(|(_, d)| **d > 0)
        .map(|(b, _)| b.clone())
        .collect();

    let live: Vec<String> = GLC_NODES
        .lock()
        .ok()
        .map(|g| g.keys().cloned().collect())
        .unwrap_or_default();
    for bus in live {
        if !want.contains(&bus) {
            teardown_node(&bus);
        }
    }

    let q = clock.quantum.max(1);
    last_quantum().store(q, Ordering::Release);
    for bus in &want {
        let pad = plan.master_pad.get(bus).copied().unwrap_or(0);
        ensure_node(bus, delay_line_samples(pad, q), clock)?;
    }

    // Drop stray post→Master when GLC owns the Master edge (arm allow-list also
    // strips this; re-assert glc→Master here).
    for bus in &want {
        let glc = glc_name_for_bus(bus);
        let _ = crate::backend::ensure_link(&glc, "buschain_master");
    }
    Ok(())
}

/// Push DelayLine sizes for live nodes after a latency-only recompute (no spawn).
pub fn apply_delays_to_live_nodes(plan: &GlcPlan) {
    let q = last_quantum().load(Ordering::Acquire).max(1);
    if let Ok(reg) = GLC_NODES.lock() {
        for (bus, node) in reg.iter() {
            let pad = plan.master_pad.get(bus).copied().unwrap_or(0);
            node.node.state.set_pdc_delay(delay_line_samples(pad, q));
        }
    }
}

fn ensure_node(bus: &str, delay_line: u32, clock: &GraphClock) -> Result<()> {
    // Node exists whenever δ>0 (caller); DelayLine may be 0 if the filter hop
    // alone covers the pad.
    let name = glc_name_for_bus(bus);
    {
        let reg = GLC_NODES.lock().unwrap();
        if let Some(n) = reg.get(bus) {
            if n.node.is_running() {
                n.node.state.set_pdc_delay(delay_line);
                if let Ok(mut st) = GLC_STATE.lock() {
                    st.insert(bus.to_string(), Arc::clone(&n.node.state));
                }
                drop(reg);
                let _ = crate::backend::ensure_link(&name, "buschain_master");
                return Ok(());
            }
        }
    }

    let node = PwFxNode::start_named(bus, &name, clock_sr(clock), clock_q(clock))?;
    node.state.set_pdc_delay(delay_line);
    if let Ok(mut st) = GLC_STATE.lock() {
        st.insert(bus.to_string(), Arc::clone(&node.state));
    }
    let _ = crate::backend::ensure_link(&name, "buschain_master");
    let mut reg = GLC_NODES.lock().unwrap();
    if let Some(old) = reg.insert(bus.to_string(), GlcNode { node }) {
        drop(old);
    }
    Ok(())
}

pub fn teardown_node(bus: &str) {
    let old = {
        let mut reg = GLC_NODES.lock().unwrap();
        reg.remove(bus)
    };
    if let Some(n) = old {
        let name = n.node.fx_name.clone();
        let _ = crate::backend::unlink(&name, "buschain_master");
        n.node.stop();
    }
    if let Ok(mut st) = GLC_STATE.lock() {
        st.remove(bus);
    }
}

pub fn teardown_all_nodes() {
    let live: Vec<String> = GLC_NODES
        .lock()
        .ok()
        .map(|g| g.keys().cloned().collect())
        .unwrap_or_default();
    for bus in live {
        teardown_node(&bus);
    }
}

pub fn node_is_live(bus: &str) -> bool {
    GLC_NODES
        .lock()
        .ok()
        .and_then(|g| g.get(bus).map(|n| n.node.is_running()))
        .unwrap_or(false)
}

/// Recompute + sync nodes from Desired (Route / clock / EnsureTrack).
pub fn reconcile(desired: &DesiredState) -> Result<GlcPlan> {
    let plan = recompute_from_desired_report(desired)?;
    sync_nodes(&plan, &desired.clock)?;
    // Master Direct toggles host peer PDC on/off with GLC.
    crate::host::refresh_peer_pads();
    crate::host::node_latency::apply_all_pdc_delays();
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize tests that mutate process-wide GLC disabled / pad maps.
    fn global_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn nested_t1_t2_master_pads_short_path() {
        // T1 → Master, T1 → T2 → Master
        // L_local(T1)=100, L_local(T2)=50
        // L(T1)=100, L(T2)=150, L*=150 ⇒ δ(T1)=50, δ(T2)=0
        let mut locals = HashMap::new();
        locals.insert("buschain_track_a".into(), 100);
        locals.insert("buschain_track_b".into(), 50);
        let edges = vec![("buschain_track_a".into(), "buschain_track_b".into())];
        let mut feeds = HashSet::new();
        feeds.insert("buschain_track_a".into());
        feeds.insert("buschain_track_b".into());
        let plan = compute_glc(&locals, &edges, &feeds).unwrap();
        assert_eq!(plan.l_star, 150);
        assert_eq!(plan.master_pad["buschain_track_a"], 50);
        assert_eq!(plan.master_pad["buschain_track_b"], 0);
        // Path + δ equal L* for every Master feeder.
        for b in &feeds {
            let l = plan.path_latency[b];
            let d = plan.master_pad[b];
            assert_eq!(l + d, plan.l_star);
        }
    }

    #[test]
    fn no_nest_equal_locals_zero_pads() {
        let mut locals = HashMap::new();
        locals.insert("buschain_track_a".into(), 80);
        locals.insert("buschain_track_b".into(), 80);
        let edges: Vec<(String, String)> = vec![];
        let mut feeds = HashSet::new();
        feeds.insert("buschain_track_a".into());
        feeds.insert("buschain_track_b".into());
        let plan = compute_glc(&locals, &edges, &feeds).unwrap();
        assert_eq!(plan.l_star, 80);
        assert_eq!(plan.master_pad["buschain_track_a"], 0);
        assert_eq!(plan.master_pad["buschain_track_b"], 0);
    }

    #[test]
    fn diamond_aligns_all_master_paths() {
        // A → B → Master, A → C → Master, B → Master, C → Master
        let mut locals = HashMap::new();
        locals.insert("a".into(), 10);
        locals.insert("b".into(), 30);
        locals.insert("c".into(), 5);
        let edges = vec![
            ("a".into(), "b".into()),
            ("a".into(), "c".into()),
        ];
        let mut feeds = HashSet::new();
        feeds.insert("a".into());
        feeds.insert("b".into());
        feeds.insert("c".into());
        let plan = compute_glc(&locals, &edges, &feeds).unwrap();
        // L(a)=10, L(b)=40, L(c)=15, L*=40
        assert_eq!(plan.path_latency["a"], 10);
        assert_eq!(plan.path_latency["b"], 40);
        assert_eq!(plan.path_latency["c"], 15);
        assert_eq!(plan.l_star, 40);
        assert_eq!(plan.master_pad["a"], 30);
        assert_eq!(plan.master_pad["b"], 0);
        assert_eq!(plan.master_pad["c"], 25);
        for b in &feeds {
            assert_eq!(plan.path_latency[b] + plan.master_pad[b], plan.l_star);
        }
    }

    #[test]
    fn cycle_rejected() {
        let mut locals = HashMap::new();
        locals.insert("a".into(), 1);
        locals.insert("b".into(), 1);
        let edges = vec![("a".into(), "b".into()), ("b".into(), "a".into())];
        let mut feeds = HashSet::new();
        feeds.insert("a".into());
        let err = compute_glc(&locals, &edges, &feeds).unwrap_err();
        assert!(matches!(err, GlcError::Cycle(_)));
    }

    #[test]
    fn self_loop_rejected() {
        let mut locals = HashMap::new();
        locals.insert("a".into(), 1);
        let edges = vec![("a".into(), "a".into())];
        let feeds = HashSet::new();
        assert!(compute_glc(&locals, &edges, &feeds).is_err());
    }

    #[test]
    fn recompute_from_desired_nested_routes() {
        let _g = global_lock();
        use crate::domain::{NodeName, NodeRole, NodeSpec};
        use crate::plan::DesiredState;
        let mut desired = DesiredState::default();
        desired.glc_disabled = false; // enable sync for this test
        desired.clock.quantum = 64;
        desired.clock.sample_rate = 48_000;
        for (name, role) in [
            ("buschain_track_a", NodeRole::TrackBus),
            ("buschain_track_b", NodeRole::TrackBus),
            ("buschain_master", NodeRole::MasterBus),
        ] {
            desired.ensure_bus(NodeSpec {
                name: NodeName::new(name),
                description: name.into(),
                role,
                start_muted: false,
                pulse_export: false,
            });
        }
        desired.set_bus_egress(
            "buschain_track_a",
            vec!["buschain_master".into(), "buschain_track_b".into()],
        );
        desired.set_bus_egress("buschain_track_b", vec!["buschain_master".into()]);
        // No reported rack latency ⇒ L_local = STAGE_QUANTA * quantum for each.
        let plan = recompute_from_desired(&desired).unwrap();
        let local = STAGE_QUANTA * 64;
        assert_eq!(plan.path_latency["buschain_track_a"], local);
        assert_eq!(plan.path_latency["buschain_track_b"], local * 2);
        assert_eq!(plan.master_pad["buschain_track_a"], local);
        assert_eq!(plan.master_pad["buschain_track_b"], 0);
        // Physical Master dest for A uses GLC; B is direct.
        assert_eq!(
            physical_egress_dests("buschain_track_a", &["buschain_master".into()])[0],
            "buschain_glc_a"
        );
        assert_eq!(
            physical_egress_dests("buschain_track_b", &["buschain_master".into()])[0],
            "buschain_master"
        );
    }

    #[test]
    fn master_direct_disables_glc_plan() {
        let _g = global_lock();
        use crate::domain::{NodeName, NodeRole, NodeSpec};
        use crate::plan::DesiredState;
        let mut desired = DesiredState::default();
        desired.glc_disabled = true;
        desired.clock.quantum = 64;
        desired.ensure_bus(NodeSpec {
            name: NodeName::new("buschain_track_a"),
            description: "a".into(),
            role: NodeRole::TrackBus,
            start_muted: false,
            pulse_export: false,
        });
        desired.set_bus_egress("buschain_track_a", vec!["buschain_master".into()]);
        let plan = recompute_from_desired(&desired).unwrap();
        assert!(is_disabled());
        assert_eq!(plan.l_star, 0);
        assert!(plan.master_pad.is_empty());
        assert_eq!(
            physical_egress_dests("buschain_track_a", &["buschain_master".into()])[0],
            "buschain_master"
        );
    }

    #[test]
    fn direct_bus_excluded_from_sync_graph() {
        let _g = global_lock();
        use crate::domain::{NodeName, NodeRole, NodeSpec};
        use crate::plan::DesiredState;
        let mut desired = DesiredState::default();
        desired.glc_disabled = false;
        desired.clock.quantum = 64;
        for name in ["buschain_track_a", "buschain_track_b"] {
            desired.ensure_bus(NodeSpec {
                name: NodeName::new(name),
                description: name.into(),
                role: NodeRole::TrackBus,
                start_muted: false,
                pulse_export: false,
            });
        }
        // A → Master and A → B → Master, but A is Direct.
        desired.glc_direct.insert("buschain_track_a".into());
        desired.set_bus_egress(
            "buschain_track_a",
            vec!["buschain_master".into(), "buschain_track_b".into()],
        );
        desired.set_bus_egress("buschain_track_b", vec!["buschain_master".into()]);
        let plan = recompute_from_desired(&desired).unwrap();
        assert!(!plan.path_latency.contains_key("buschain_track_a"));
        assert_eq!(plan.master_pad.get("buschain_track_a").copied().unwrap_or(0), 0);
        // Only B remains as Master feeder (no nested boost from A).
        assert_eq!(
            plan.master_pad.get("buschain_track_b").copied().unwrap_or(0),
            0
        );
        assert_eq!(
            physical_egress_dests("buschain_track_a", &["buschain_master".into()])[0],
            "buschain_master"
        );
    }

    #[test]
    fn delay_line_subtracts_glc_filter_quantum() {
        assert_eq!(delay_line_samples(0, 256), 0);
        assert_eq!(delay_line_samples(256, 256), 0); // filter hop alone
        assert_eq!(delay_line_samples(512, 256), 256);
        assert_eq!(delay_line_samples(100, 256), 0);
    }

    #[test]
    fn nested_formula_matches_plan_example() {
        // T1∥T1→T2→Master ⇒ δ(T1)=L_local(T2), δ(T2)=0
        let mut locals = HashMap::new();
        locals.insert("t1".into(), 64);
        locals.insert("t2".into(), 192);
        let edges = vec![("t1".into(), "t2".into())];
        let feeds = HashSet::from(["t1".into(), "t2".into()]);
        let plan = compute_glc(&locals, &edges, &feeds).unwrap();
        assert_eq!(plan.master_pad["t1"], 192);
        assert_eq!(plan.master_pad["t2"], 0);
        assert_eq!(plan.l_star, 64 + 192);
    }

    #[test]
    fn track_to_track_never_rewritten_to_glc() {
        let _g = global_lock();
        set_disabled(false);
        publish_plan(&GlcPlan {
            path_latency: HashMap::from([
                ("buschain_track_a".into(), 10),
                ("buschain_track_b".into(), 40),
            ]),
            l_star: 40,
            master_pad: HashMap::from([
                ("buschain_track_a".into(), 30),
                ("buschain_track_b".into(), 0),
            ]),
        });
        let dests = physical_egress_dests(
            "buschain_track_a",
            &[
                "buschain_master".into(),
                "buschain_track_b".into(),
                "buschain_vinf_a".into(),
            ],
        );
        assert_eq!(dests[0], "buschain_glc_a");
        assert_eq!(dests[1], "buschain_track_b");
        assert_eq!(dests[2], "buschain_vinf_a");
    }

    #[test]
    fn zero_pad_means_no_glc_physical_dest() {
        let _g = global_lock();
        publish_plan(&GlcPlan {
            path_latency: HashMap::from([("buschain_track_x".into(), 100)]),
            l_star: 100,
            master_pad: HashMap::from([("buschain_track_x".into(), 0)]),
        });
        assert_eq!(
            physical_egress_dests("buschain_track_x", &["buschain_master".into()]),
            vec!["buschain_master".to_string()]
        );
        // Contract: δ=0 ⇒ no buschain_glc_* node required.
        assert!(!node_is_live("buschain_track_x"));
    }

    #[test]
    fn physical_dest_uses_glc_only_when_pad_positive() {
        let _g = global_lock();
        set_disabled(false);
        publish_plan(&GlcPlan {
            path_latency: HashMap::from([("buschain_track_a".into(), 10)]),
            l_star: 20,
            master_pad: HashMap::from([("buschain_track_a".into(), 10)]),
        });
        let dests = physical_egress_dests(
            "buschain_track_a",
            &["buschain_master".into(), "buschain_track_b".into()],
        );
        assert_eq!(dests[0], "buschain_glc_a");
        assert_eq!(dests[1], "buschain_track_b");

        publish_plan(&GlcPlan {
            path_latency: HashMap::from([("buschain_track_a".into(), 20)]),
            l_star: 20,
            master_pad: HashMap::from([("buschain_track_a".into(), 0)]),
        });
        let dests = physical_egress_dests("buschain_track_a", &["buschain_master".into()]);
        assert_eq!(dests[0], "buschain_master");
        assert!(master_uses_direct_link("buschain_track_a"));
    }

    /// Property: for deterministic DAG family, path+δ = L* for every Master feeder.
    #[test]
    fn prop_path_plus_delta_equals_lstar() {
        // Seeded LCG over forward-only edges (i → j only if i < j) ⇒ DAG.
        let mut seed: u64 = 0xC0FFEE_u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1);
            seed
        };
        for case in 0..64 {
            let n = (next() as usize % 16) + 1;
            let mut locals = HashMap::new();
            let names: Vec<String> = (0..n).map(|i| format!("t{i}")).collect();
            for name in &names {
                locals.insert(name.clone(), (next() % 200) as u32);
            }
            let mut edges = Vec::new();
            for i in 0..n {
                for j in (i + 1)..n {
                    if next() % 4 == 0 {
                        edges.push((names[i].clone(), names[j].clone()));
                    }
                }
            }
            let mut feeds = HashSet::new();
            for name in &names {
                if next() % 5 < 3 {
                    feeds.insert(name.clone());
                }
            }
            if feeds.is_empty() {
                feeds.insert(names[0].clone());
            }
            let plan = compute_glc(&locals, &edges, &feeds).unwrap_or_else(|e| {
                panic!("case {case} DAG failed: {e}");
            });
            for b in &feeds {
                assert_eq!(
                    plan.path_latency[b] + plan.master_pad[b],
                    plan.l_star,
                    "case {case} bus {b}"
                );
            }
        }
    }
}
