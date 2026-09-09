//! Graph clock domains — BusChain engine rate/quantum (independent of Master HW).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::domain::DeviceNode;

/// `pw-dump` is huge — never spawn it unboundedly (UI used to call this every frame).
const PW_DUMP_TTL: Duration = Duration::from_millis(800);
/// Device EnumFormat/ALSA rate lists change rarely.
const DEVICE_RATES_TTL: Duration = Duration::from_secs(8);
/// Short `pactl list short` rate map (one process for all nodes).
const PACTL_SHORT_TTL: Duration = Duration::from_millis(400);

fn pw_dump_cache() -> &'static Mutex<Option<(Instant, serde_json::Value)>> {
    static C: OnceLock<Mutex<Option<(Instant, serde_json::Value)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

fn device_rates_cache() -> &'static Mutex<HashMap<String, (Instant, Vec<u32>)>> {
    static C: OnceLock<Mutex<HashMap<String, (Instant, Vec<u32>)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pactl_short_cache() -> &'static Mutex<HashMap<&'static str, (Instant, HashMap<String, u32>)>> {
    static C: OnceLock<Mutex<HashMap<&'static str, (Instant, HashMap<String, u32>)>>> =
        OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Drop cached dumps/rates after a successful clock bind so the next probe sees truth.
pub fn invalidate_clock_probe_caches() {
    if let Ok(mut g) = pw_dump_cache().lock() {
        *g = None;
    }
    if let Ok(mut g) = device_rates_cache().lock() {
        g.clear();
    }
    if let Ok(mut g) = pactl_short_cache().lock() {
        g.clear();
    }
}

fn clock_mutation_until() -> &'static Mutex<Option<Instant>> {
    static C: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// Hollow detect must not fire `ReconnectPipeWire` while buses are mid-migrate.
pub fn mark_clock_mutation(hold: Duration) {
    if let Ok(mut g) = clock_mutation_until().lock() {
        let until = Instant::now() + hold;
        *g = Some(g.map(|prev| prev.max(until)).unwrap_or(until));
    }
}

pub fn clock_mutation_in_flight() -> bool {
    match clock_mutation_until().lock() {
        Ok(g) => g.is_some_and(|until| Instant::now() < until),
        Err(_) => false,
    }
}

pub fn clear_clock_mutation() {
    if let Ok(mut g) = clock_mutation_until().lock() {
        *g = None;
    }
}

/// User-facing performance preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AudioPreset {
    #[default]
    Balanced,
    LowLatency,
    Stable,
    Custom,
}

impl AudioPreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::Balanced => "Balanced",
            Self::LowLatency => "Low latency",
            Self::Stable => "Stable",
            Self::Custom => "Custom",
        }
    }
}

/// Probed capabilities of the Master HW out (or fallback device).
#[derive(Debug, Clone, Default)]
pub struct DeviceCaps {
    pub sink_name: String,
    pub description: String,
    pub rates: Vec<u32>,
    pub preferred_rate: u32,
    pub min_quantum: u32,
    pub max_quantum: u32,
    pub preferred_quantum: u32,
}

impl DeviceCaps {
    pub fn period_ms(&self, rate: u32, quantum: u32) -> f32 {
        if rate == 0 {
            return 0.0;
        }
        (quantum as f32 * 1000.0) / rate as f32
    }

    /// Empty rate list ⇒ only `preferred_rate` (never “any rate”).
    pub fn allows_rate(&self, rate: u32) -> bool {
        if self.rates.is_empty() {
            return rate > 0 && rate == self.preferred_rate;
        }
        self.rates.contains(&rate)
    }

    pub fn allows_quantum(&self, q: u32) -> bool {
        q >= self.min_quantum && q <= self.max_quantum && q.is_power_of_two()
    }
}

/// Resolved profile applied to BusChain-owned nodes (= session `performance`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceProfile {
    pub preset: AudioPreset,
    pub sample_rate: u32,
    pub quantum: u32,
    pub soft_quantum: bool,
    pub force_suspend_timeout_zero: bool,
    /// Sink this profile was computed for (stale detection).
    pub bound_device: String,
    /// True when we had to clamp away from the ideal / requested target.
    #[serde(default)]
    pub device_limited: bool,
}

impl Default for PerformanceProfile {
    fn default() -> Self {
        Self {
            preset: AudioPreset::Balanced,
            sample_rate: 48_000,
            quantum: 256,
            soft_quantum: true,
            force_suspend_timeout_zero: true,
            bound_device: String::new(),
            device_limited: false,
        }
    }
}

impl PerformanceProfile {
    pub fn period_ms(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        (self.quantum as f32 * 1000.0) / self.sample_rate as f32
    }

    /// `node.latency` style string, e.g. `"256/48000"`.
    pub fn node_latency_prop(&self) -> String {
        format!("{}/{}", self.quantum, self.sample_rate)
    }

    /// View as the live graph clock.
    pub fn graph_clock(&self) -> GraphClock {
        GraphClock {
            sample_rate: self.sample_rate.max(1),
            quantum: self.quantum.max(1),
            soft_quantum: self.soft_quantum,
            force_suspend_timeout_zero: self.force_suspend_timeout_zero,
            bound_device: self.bound_device.clone(),
        }
    }
}

/// Active clock for all BusChain-owned nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphClock {
    pub sample_rate: u32,
    pub quantum: u32,
    pub soft_quantum: bool,
    pub force_suspend_timeout_zero: bool,
    pub bound_device: String,
}

impl Default for GraphClock {
    fn default() -> Self {
        PerformanceProfile::default().graph_clock()
    }
}

impl GraphClock {
    pub fn node_latency_prop(&self) -> String {
        format!("{}/{}", self.quantum, self.sample_rate)
    }

    pub fn period_ms(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        (self.quantum as f32 * 1000.0) / self.sample_rate as f32
    }
}

/// Native caps of an external (or BusChain) endpoint.
#[derive(Debug, Clone, Default)]
pub struct EndpointCaps {
    pub name: String,
    pub rate: Option<u32>,
    pub channels: Option<u32>,
}

/// Safe fallback when no device is found (not a capability advertisement).
const FALLBACK_RATES: &[u32] = &[48_000, 44_100, 96_000, 88_200];

/// BusChain engine sample rates (Settings → Audio). Not clamped to Master HW caps.
/// Includes DXD (352.8) and 384 kHz — archival / audiophile apex standards.
pub const ENGINE_RATES: &[u32] = &[
    44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000,
];

/// Rates above this are valid but extreme (CPU / plugin risk) — UI should warn.
pub const ENGINE_RATE_WARN_ABOVE: u32 = 192_000;

/// BusChain engine quantum choices (powers of two).
pub const ENGINE_QUANTUMS: &[u32] = &[64, 128, 256, 512, 1024, 2048];

/// True for DXD / 384k-class engine rates (show a soft warning in Settings).
pub fn engine_rate_is_extreme(rate: u32) -> bool {
    rate > ENGINE_RATE_WARN_ABOVE
}

fn engine_allows_rate(rate: u32) -> bool {
    ENGINE_RATES.contains(&rate)
}

fn engine_allows_quantum(q: u32) -> bool {
    ENGINE_QUANTUMS.contains(&q)
}

fn snap_engine_rate(rate: u32) -> u32 {
    ENGINE_RATES
        .iter()
        .copied()
        .min_by_key(|c| rate.abs_diff(*c))
        .unwrap_or(48_000)
}

fn snap_engine_quantum(q: u32) -> u32 {
    let q = q.max(1).next_power_of_two();
    ENGINE_QUANTUMS
        .iter()
        .copied()
        .min_by_key(|c| q.abs_diff(*c))
        .unwrap_or(256)
}

/// Resolve BusChain engine profile (independent of Master HW DeviceCaps).
pub fn resolve_engine_profile(
    preset: AudioPreset,
    custom_rate: Option<u32>,
    custom_quantum: Option<u32>,
    soft_quantum: bool,
) -> PerformanceProfile {
    let (rate, quantum) = match preset {
        AudioPreset::Balanced => (48_000, 256),
        AudioPreset::LowLatency => (48_000, 64),
        AudioPreset::Stable => (48_000, 512),
        AudioPreset::Custom => {
            let rate = custom_rate
                .filter(|r| engine_allows_rate(*r))
                .unwrap_or_else(|| {
                    custom_rate
                        .map(snap_engine_rate)
                        .unwrap_or(48_000)
                });
            let quantum = custom_quantum
                .filter(|q| engine_allows_quantum(*q))
                .unwrap_or_else(|| {
                    custom_quantum
                        .map(snap_engine_quantum)
                        .unwrap_or(256)
                });
            (rate, quantum)
        }
    };
    PerformanceProfile {
        preset,
        sample_rate: rate.max(1),
        quantum: quantum.max(1),
        soft_quantum,
        force_suspend_timeout_zero: true,
        bound_device: String::new(),
        device_limited: false,
    }
}

/// Probe Master HW out from an already-listed sink table.
pub fn probe_master_hw_from_sinks(
    sinks: &[DeviceNode],
    master_output: Option<&str>,
) -> DeviceCaps {
    let chosen = pick_hw_sink(sinks, master_output);
    let Some(sink) = chosen else {
        return DeviceCaps {
            sink_name: String::new(),
            description: "(no hardware sink)".into(),
            rates: FALLBACK_RATES.to_vec(),
            preferred_rate: 48_000,
            min_quantum: 64,
            max_quantum: 2048,
            preferred_quantum: 256,
        };
    };

    let mut rates = probe_true_device_rates(&sink.name);
    let running = probe_sink_running_rate(&sink.name);
    let preferred_rate = running
        .or_else(|| rates.first().copied())
        .unwrap_or(48_000);
    if rates.is_empty() {
        rates.push(preferred_rate);
    }
    // Put running rate first for Balanced / UI default ordering.
    if let Some(r) = running {
        rates.retain(|x| *x != r);
        rates.insert(0, r);
    }
    let preferred_quantum = quantum_for_budget(preferred_rate, 6.0, 64, 2048);
    DeviceCaps {
        sink_name: sink.name.clone(),
        description: sink.description.clone(),
        rates,
        preferred_rate,
        min_quantum: 64,
        max_quantum: 2048,
        preferred_quantum,
    }
}

fn pick_hw_sink<'a>(sinks: &'a [DeviceNode], master_output: Option<&str>) -> Option<&'a DeviceNode> {
    if let Some(name) = master_output {
        if let Some(s) = sinks
            .iter()
            .find(|s| s.name == name && !is_buschain_helper(&s.name))
        {
            return Some(s);
        }
    }
    sinks.iter().find(|s| {
        !is_buschain_helper(&s.name)
            && !s.name.starts_with("buschain_")
            && !s.name.contains("auto_null")
    })
}

fn is_buschain_helper(name: &str) -> bool {
    name.starts_with("buschain_fx_")
        || name.starts_with("buschain_post_")
        || name.starts_with("buschain_mid_")
        || name.starts_with("buschain_glc_")
        || name.starts_with("buschain_rs_")
        || name == "buschain_hold"
}

fn quantum_for_budget(rate: u32, target_ms: f32, min_q: u32, max_q: u32) -> u32 {
    if rate == 0 {
        return 256;
    }
    let ideal = ((target_ms / 1000.0) * rate as f32).round() as u32;
    let mut q = ideal.next_power_of_two() / 2;
    if q < min_q {
        q = min_q;
    }
    let max_samples = ((8.0 / 1000.0) * rate as f32) as u32;
    while q * 2 <= max_q && q * 2 <= max_samples.max(min_q) {
        q *= 2;
    }
    while q > max_q {
        q /= 2;
    }
    q.clamp(min_q, max_q)
        .next_power_of_two()
        .min(max_q)
        .max(min_q)
}

/// Resolve a preset against probed device capabilities.
pub fn resolve_profile(
    preset: AudioPreset,
    caps: &DeviceCaps,
    custom_rate: Option<u32>,
    custom_quantum: Option<u32>,
    soft_quantum: bool,
) -> PerformanceProfile {
    let rate = match preset {
        AudioPreset::Custom => custom_rate
            .filter(|r| caps.allows_rate(*r))
            .unwrap_or(caps.preferred_rate.max(1)),
        _ => pick_rate(caps),
    };

    let (quantum, device_limited) = match preset {
        AudioPreset::Balanced => {
            let ideal = quantum_for_budget(rate, 6.0, caps.min_quantum, caps.max_quantum);
            let q = ideal.clamp(caps.min_quantum, caps.max_quantum);
            let limited = q != ideal || rate != 48_000;
            (q, limited)
        }
        AudioPreset::LowLatency => {
            let bal = quantum_for_budget(rate, 6.0, caps.min_quantum, caps.max_quantum);
            let want = (bal / 2).max(caps.min_quantum).next_power_of_two();
            let q = want.clamp(caps.min_quantum, caps.max_quantum);
            (
                q,
                q > want || (q == caps.min_quantum && want < caps.min_quantum),
            )
        }
        AudioPreset::Stable => {
            let bal = quantum_for_budget(rate, 6.0, caps.min_quantum, caps.max_quantum);
            let want = (bal * 2).min(caps.max_quantum).next_power_of_two();
            let q = want.clamp(caps.min_quantum, caps.max_quantum);
            (q, false)
        }
        AudioPreset::Custom => {
            let want_rate = custom_rate.unwrap_or(rate);
            let want_q = custom_quantum.unwrap_or(caps.preferred_quantum);
            let q = custom_quantum
                .filter(|q| caps.allows_quantum(*q))
                .unwrap_or(caps.preferred_quantum);
            // Limited only when the user's request was clamped away.
            let limited = (custom_rate.is_some() && want_rate != rate)
                || (custom_quantum.is_some() && want_q != q);
            (q, limited)
        }
    };

    PerformanceProfile {
        preset,
        sample_rate: rate.max(1),
        quantum: quantum.max(1),
        soft_quantum,
        force_suspend_timeout_zero: true,
        bound_device: caps.sink_name.clone(),
        device_limited,
    }
}

fn pick_rate(caps: &DeviceCaps) -> u32 {
    if caps.preferred_rate > 0 && caps.allows_rate(caps.preferred_rate) {
        return caps.preferred_rate;
    }
    for &r in FALLBACK_RATES {
        if caps.allows_rate(r) {
            return r;
        }
    }
    caps.rates.first().copied().unwrap_or(48_000)
}

/// Live sample rate — prefer ALSA momentary / PW Format when pactl lags after force-rate.
pub fn probe_sink_running_rate(name: &str) -> Option<u32> {
    // Hardware: ALSA "Momentary freq" is truth after clock.force-rate (pactl often stale).
    if !name.starts_with("buschain_") {
        if let Some(r) = probe_alsa_momentary_rate(name) {
            return Some(r);
        }
        if let Some(r) = probe_pw_format_rate(name) {
            return Some(r);
        }
    }
    probe_pactl_running_rate("sinks", name).or_else(|| probe_pw_format_rate(name))
}

fn probe_alsa_momentary_rate(sink_name: &str) -> Option<u32> {
    let card = probe_alsa_card_for_node(sink_name)?;
    let path = format!("/proc/asound/card{card}/stream0");
    let text = std::fs::read_to_string(path).ok()?;
    // "Momentary freq = 95999 Hz" or "48000 Hz"
    for line in text.lines() {
        if let Some(rest) = line.split("Momentary freq").nth(1) {
            for tok in rest.split_whitespace() {
                if let Ok(r) = tok.parse::<u32>() {
                    if (8_000..=384_000).contains(&r) {
                        // USB often reports 95999 — snap to nearest common rate.
                        return Some(snap_common_rate(r));
                    }
                }
            }
        }
    }
    None
}

fn snap_common_rate(r: u32) -> u32 {
    const CAND: &[u32] = &[
        8_000, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
        352_800, 384_000,
    ];
    CAND
        .iter()
        .copied()
        .min_by_key(|c| r.abs_diff(*c))
        .unwrap_or(r)
}

fn probe_pw_format_rate(node_name: &str) -> Option<u32> {
    let dump = pw_dump_json()?;
    let arr = dump.as_array()?;
    for o in arr {
        let Some(props) = o.pointer("/info/props") else {
            continue;
        };
        let Some(name) = props.get("node.name").and_then(|v| v.as_str()) else {
            continue;
        };
        if name != node_name {
            continue;
        }
        let Some(params) = o.pointer("/info/params") else {
            continue;
        };
        // Current Format rate (not Enum min/max).
        if let Some(fmt) = params.get("Format").and_then(|v| v.as_array()) {
            for item in fmt {
                if let Some(r) = item.get("rate").and_then(json_u32) {
                    if (8_000..=384_000).contains(&r) {
                        return Some(r);
                    }
                }
            }
        }
    }
    None
}

/// Live sample rate from `pactl list short sources`.
pub fn probe_source_running_rate(name: &str) -> Option<u32> {
    let node = name.strip_suffix(".monitor").unwrap_or(name);
    probe_pactl_running_rate("sources", node)
        .or_else(|| probe_pactl_running_rate("sources", &format!("{node}.monitor")))
}

fn probe_pactl_running_rate(kind: &str, name: &str) -> Option<u32> {
    let kind_key: &'static str = match kind {
        "sinks" => "sinks",
        "sources" => "sources",
        _ => return None,
    };
    let map = pactl_short_rate_map(kind_key)?;
    map.get(name).copied()
}

fn pactl_short_rate_map(kind: &'static str) -> Option<HashMap<String, u32>> {
    if let Ok(g) = pactl_short_cache().lock() {
        if let Some((at, map)) = g.get(kind) {
            if at.elapsed() < PACTL_SHORT_TTL {
                return Some(map.clone());
            }
        }
    }
    let out = std::process::Command::new("pactl")
        .args(["list", "short", kind])
        .output()
        .ok()?;
    let mut map = HashMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split('\t');
        let _idx = parts.next();
        let Some(n) = parts.next() else { continue };
        let _driver = parts.next();
        let spec = parts.next().unwrap_or("");
        for tok in spec.split_whitespace() {
            if let Some(hz) = tok.strip_suffix("Hz") {
                if let Ok(r) = hz.parse::<u32>() {
                    if r >= 8_000 {
                        map.insert(n.to_string(), r);
                        break;
                    }
                }
            }
        }
    }
    if let Ok(mut g) = pactl_short_cache().lock() {
        g.insert(kind, (Instant::now(), map.clone()));
    }
    Some(map)
}

/// True device rates: EnumFormat on related PW nodes ∪ ALSA `Rates:` for the card.
/// Never invents 192k from bare substring matches in unrelated props.
pub fn probe_true_device_rates(sink_name: &str) -> Vec<u32> {
    if let Ok(g) = device_rates_cache().lock() {
        if let Some((at, rates)) = g.get(sink_name) {
            if at.elapsed() < DEVICE_RATES_TTL {
                return rates.clone();
            }
        }
    }
    let rates = probe_true_device_rates_uncached(sink_name);
    if let Ok(mut g) = device_rates_cache().lock() {
        g.insert(sink_name.to_string(), (Instant::now(), rates.clone()));
    }
    rates
}

fn probe_true_device_rates_uncached(sink_name: &str) -> Vec<u32> {
    let mut set = BTreeSet::new();
    if let Some((card, enum_rates)) = probe_rates_pwdump_structured(sink_name) {
        set.extend(enum_rates);
        if let Some(c) = card {
            set.extend(probe_rates_alsa_card(c));
        }
    } else if let Some(card) = probe_alsa_card_for_node(sink_name) {
        set.extend(probe_rates_alsa_card(card));
    }
    // Also merge EnumFormat from internal hw node sharing the card.
    set.extend(probe_enum_rates_for_sink_family(sink_name));
    set.into_iter()
        .filter(|r| (8_000..=384_000).contains(r))
        .collect()
}

/// Best-effort sample-rate list (legacy name) — now structured, not substring tokens.
pub fn probe_rates_pw(node_name: &str) -> Vec<u32> {
    probe_true_device_rates(node_name)
}

fn probe_alsa_card_for_node(node_name: &str) -> Option<u32> {
    let dump = pw_dump_json()?;
    find_alsa_card_in_dump(&dump, node_name)
}

fn probe_rates_pwdump_structured(sink_name: &str) -> Option<(Option<u32>, Vec<u32>)> {
    let dump = pw_dump_json()?;
    let card = find_alsa_card_in_dump(&dump, sink_name);
    let mut rates = BTreeSet::new();
    collect_enum_rates_for_name(&dump, sink_name, &mut rates);
    if let Some(c) = card {
        // Sibling nodes on same card (e.g. alsa_output.hw_USB_0).
        collect_enum_rates_for_card(&dump, c, &mut rates);
    }
    Some((card, rates.into_iter().collect()))
}

fn probe_enum_rates_for_sink_family(sink_name: &str) -> Vec<u32> {
    let Some(dump) = pw_dump_json() else {
        return Vec::new();
    };
    let mut rates = BTreeSet::new();
    collect_enum_rates_for_name(&dump, sink_name, &mut rates);
    if let Some(card) = find_alsa_card_in_dump(&dump, sink_name) {
        collect_enum_rates_for_card(&dump, card, &mut rates);
    }
    rates.into_iter().collect()
}

fn pw_dump_json() -> Option<serde_json::Value> {
    if let Ok(g) = pw_dump_cache().lock() {
        if let Some((at, value)) = g.as_ref() {
            if at.elapsed() < PW_DUMP_TTL {
                return Some(value.clone());
            }
        }
    }
    let out = std::process::Command::new("pw-dump").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    if let Ok(mut g) = pw_dump_cache().lock() {
        *g = Some((Instant::now(), value.clone()));
    }
    Some(value)
}

fn json_u32(v: &serde_json::Value) -> Option<u32> {
    v.as_u64()
        .map(|n| n as u32)
        .or_else(|| v.as_i64().map(|n| n as u32))
        .or_else(|| v.as_str()?.parse().ok())
}

fn find_alsa_card_in_dump(dump: &serde_json::Value, node_name: &str) -> Option<u32> {
    let arr = dump.as_array()?;
    for o in arr {
        // Must continue — early `?` aborts the whole scan on the first non-node object.
        let Some(props) = o.pointer("/info/props") else {
            continue;
        };
        let Some(name) = props.get("node.name").and_then(|v| v.as_str()) else {
            continue;
        };
        if name != node_name {
            continue;
        }
        if let Some(c) = props.get("alsa.card").and_then(json_u32) {
            return Some(c);
        }
        if let Some(c) = props.get("api.alsa.card").and_then(json_u32) {
            return Some(c);
        }
        // "hw:USB,0" → try matching /proc/asound/*/id
        if let Some(path) = props.get("api.alsa.path").and_then(|v| v.as_str()) {
            if let Some(card) = resolve_alsa_card_from_path(path) {
                return Some(card);
            }
        }
    }
    None
}

fn resolve_alsa_card_from_path(path: &str) -> Option<u32> {
    // hw:USB,0 or hw:0,0
    let rest = path.strip_prefix("hw:")?;
    let id = rest.split(',').next()?;
    if let Ok(n) = id.parse::<u32>() {
        return Some(n);
    }
    // Match card id file
    if let Ok(dirs) = std::fs::read_dir("/proc/asound") {
        for ent in dirs.flatten() {
            let name = ent.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("card") {
                continue;
            }
            let Ok(num) = name.trim_start_matches("card").parse::<u32>() else {
                continue;
            };
            let id_path = ent.path().join("id");
            if let Ok(s) = std::fs::read_to_string(&id_path) {
                if s.trim() == id {
                    return Some(num);
                }
            }
        }
    }
    None
}

fn collect_enum_rates_for_name(
    dump: &serde_json::Value,
    node_name: &str,
    out: &mut BTreeSet<u32>,
) {
    let Some(arr) = dump.as_array() else {
        return;
    };
    for o in arr {
        let Some(props) = o.pointer("/info/props") else {
            continue;
        };
        let Some(name) = props.get("node.name").and_then(|v| v.as_str()) else {
            continue;
        };
        if name != node_name {
            continue;
        }
        extract_enum_rates(o.pointer("/info/params"), out);
    }
}

fn collect_enum_rates_for_card(dump: &serde_json::Value, card: u32, out: &mut BTreeSet<u32>) {
    let Some(arr) = dump.as_array() else {
        return;
    };
    for o in arr {
        let Some(props) = o.pointer("/info/props") else {
            continue;
        };
        let card_match = props.get("alsa.card").and_then(json_u32) == Some(card)
            || props.get("api.alsa.card").and_then(json_u32) == Some(card);
        if !card_match {
            continue;
        }
        extract_enum_rates(o.pointer("/info/params"), out);
    }
}

fn extract_enum_rates(params: Option<&serde_json::Value>, out: &mut BTreeSet<u32>) {
    let Some(params) = params else {
        return;
    };
    for key in ["EnumFormat", "Format"] {
        let Some(arr) = params.get(key).and_then(|v| v.as_array()) else {
            continue;
        };
        for item in arr {
            collect_rate_value(item.get("rate"), out);
        }
    }
}

fn collect_rate_value(v: Option<&serde_json::Value>, out: &mut BTreeSet<u32>) {
    let Some(v) = v else {
        return;
    };
    if let Some(n) = json_u32(v) {
        if (8_000..=384_000).contains(&n) {
            out.insert(n);
        }
        return;
    }
    if let Some(arr) = v.as_array() {
        for x in arr {
            collect_rate_value(Some(x), out);
        }
        return;
    }
    if let Some(obj) = v.as_object() {
        // PipeWire often advertises { default, min, max } — keep all three, and
        // expand common rates that fall inside [min, max].
        let min = obj.get("min").and_then(json_u32);
        let max = obj.get("max").and_then(json_u32);
        for (_k, x) in obj {
            collect_rate_value(Some(x), out);
        }
        if let (Some(lo), Some(hi)) = (min, max) {
            for &r in FALLBACK_RATES {
                if r >= lo && r <= hi {
                    out.insert(r);
                }
            }
        }
    }
}

/// Parse `Rates: 44100, 48000, …` from `/proc/asound/cardN/stream*`.
pub fn probe_rates_alsa_card(card: u32) -> Vec<u32> {
    let dir = format!("/proc/asound/card{card}");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut set = BTreeSet::new();
    for ent in entries.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("stream") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(ent.path()) else {
            continue;
        };
        for line in text.lines() {
            let Some(rest) = line.trim().strip_prefix("Rates:") else {
                continue;
            };
            for tok in rest.split(|c: char| !c.is_ascii_digit()) {
                if tok.is_empty() {
                    continue;
                }
                if let Ok(r) = tok.parse::<u32>() {
                    if (8_000..=384_000).contains(&r) {
                        set.insert(r);
                    }
                }
            }
        }
    }
    set.into_iter().collect()
}

/// Set PipeWire graph force-rate / force-quantum (settings metadata).
/// Pass `rate=0` / `quantum=0` to clear the force.
pub fn set_graph_force_clock(rate: u32, quantum: u32) -> Result<(), String> {
    set_settings_meta("clock.force-rate", &rate.to_string())?;
    set_settings_meta("clock.force-quantum", &quantum.to_string())?;
    Ok(())
}

/// Release session-wide `clock.force-*` so desktop audio is not stuck after Quit.
pub fn clear_graph_force_clock() -> Result<(), String> {
    set_graph_force_clock(0, 0)
}

fn set_settings_meta(key: &str, value: &str) -> Result<(), String> {
    let out = std::process::Command::new("pw-metadata")
        .args(["-n", "settings", "0", key, value])
        .output()
        .map_err(|e| format!("pw-metadata: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("pw-metadata {key}: {err}"));
    }
    Ok(())
}

/// Wait until Master HW running rate matches `want_rate` (bounded).
pub fn wait_hw_running_rate(hw_sink: &str, want_rate: u32, max_wait: Duration) -> bool {
    if hw_sink.is_empty() || want_rate == 0 {
        return false;
    }
    let deadline = Instant::now() + max_wait;
    while Instant::now() < deadline {
        if probe_sink_running_rate(hw_sink) == Some(want_rate) {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    probe_sink_running_rate(hw_sink) == Some(want_rate)
}

/// Probe a single endpoint's preferred / running rate.
pub fn probe_endpoint_caps(name: &str) -> EndpointCaps {
    let node = name.strip_suffix(".monitor").unwrap_or(name);
    let running = probe_source_running_rate(node)
        .or_else(|| probe_sink_running_rate(node));
    let rate = running.or_else(|| {
        // Structured enum first rate only as last resort (not substring soup).
        probe_enum_rates_for_sink_family(node)
            .into_iter()
            .next()
    });
    EndpointCaps {
        name: node.to_string(),
        rate,
        channels: Some(2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_profile_allows_192k_independent_of_hw() {
        let p = resolve_engine_profile(AudioPreset::Custom, Some(192_000), Some(256), true);
        assert_eq!(p.sample_rate, 192_000);
        assert_eq!(p.quantum, 256);
        assert!(!p.device_limited);
    }

    #[test]
    fn engine_profile_allows_dxd_and_384k() {
        let dxd = resolve_engine_profile(AudioPreset::Custom, Some(352_800), Some(512), true);
        assert_eq!(dxd.sample_rate, 352_800);
        assert!(engine_rate_is_extreme(dxd.sample_rate));
        let apex = resolve_engine_profile(AudioPreset::Custom, Some(384_000), Some(512), true);
        assert_eq!(apex.sample_rate, 384_000);
        assert!(engine_rate_is_extreme(apex.sample_rate));
        assert!(!engine_rate_is_extreme(192_000));
    }

    #[test]
    fn engine_presets_are_catalog_defaults() {
        let bal = resolve_engine_profile(AudioPreset::Balanced, None, None, true);
        assert_eq!((bal.sample_rate, bal.quantum), (48_000, 256));
        let low = resolve_engine_profile(AudioPreset::LowLatency, None, None, false);
        assert_eq!((low.sample_rate, low.quantum), (48_000, 64));
        let stab = resolve_engine_profile(AudioPreset::Stable, None, None, true);
        assert_eq!((stab.sample_rate, stab.quantum), (48_000, 512));
    }

    #[test]
    fn engine_custom_snaps_unknown_rate() {
        let p = resolve_engine_profile(AudioPreset::Custom, Some(50_000), Some(200), true);
        assert!(ENGINE_RATES.contains(&p.sample_rate));
        assert!(ENGINE_QUANTUMS.contains(&p.quantum));
    }

    #[test]
    fn clock_mutation_flag_holds_then_clears() {
        clear_clock_mutation();
        mark_clock_mutation(Duration::from_secs(2));
        assert!(clock_mutation_in_flight());
        clear_clock_mutation();
        assert!(!clock_mutation_in_flight());
    }
}
