//! Native PipeWire Links for BusChain internal hops (replaces module-loopback).
//!
//! Pulse uses `bus.monitor` source names; PipeWire ports are:
//!   `{bus}:monitor_FL/FR`  →  `{dest}:playback_FL/FR`
//! Never pass `*.monitor` as a pw-link *node* name.

use std::collections::HashSet;
use std::process::Command;
use std::sync::{LazyLock, Mutex};

use anyhow::{anyhow, Context, Result};

use super::cli::{self, CLI_TIMEOUT};

/// Owned BusChain route links: (pulse_source, pulse_sink).
static OWNED: LazyLock<Mutex<HashSet<(String, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn run_capture(bin: &str, args: &[&str]) -> Result<String> {
    cli::run_capture(bin, args, CLI_TIMEOUT)
}

fn run_status(bin: &str, args: &[&str]) -> Result<()> {
    cli::run_status(bin, args, CLI_TIMEOUT)
}

/// Strip Pulse `.monitor` suffix → PipeWire node name.
fn pw_node(endpoint: &str) -> String {
    endpoint
        .strip_suffix(".monitor")
        .unwrap_or(endpoint)
        .to_string()
}

fn is_monitor_source(source: &str) -> bool {
    source.ends_with(".monitor")
}

/// Collect port names whose node prefix matches `node`.
fn ports_for_node(listing: &str, node: &str) -> Vec<String> {
    let prefix = format!("{node}:");
    listing
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && (l.starts_with(&prefix) || *l == node))
        .map(|s| s.to_string())
        .collect()
}

fn pick_lr(ports: &[String], prefer_monitor: bool) -> Option<(String, String)> {
    let mut candidates: Vec<&String> = Vec::new();
    for p in ports {
        let pl = p.to_lowercase();
        // Prefer the right port class when both exist on a node.
        if prefer_monitor && !pl.contains("monitor") && (pl.contains("playback") || pl.contains("input"))
        {
            continue;
        }
        if !prefer_monitor && pl.contains("monitor") {
            continue;
        }
        candidates.push(p);
    }
    if candidates.is_empty() {
        candidates = ports.iter().collect();
    }

    // Scarlett Mic1/Mic2: capture_MONO → both stereo destinations.
    if let Some(mono) = candidates.iter().find(|p| p.to_lowercase().contains("mono")) {
        return Some(((*mono).clone(), (*mono).clone()));
    }
    if candidates.len() == 1 {
        return Some((candidates[0].clone(), candidates[0].clone()));
    }

    let mut fl = None;
    let mut fr = None;
    for p in &candidates {
        let pl = p.to_lowercase();
        if pl.contains("mono") {
            continue;
        }
        let is_fl = pl.contains("fl")
            || pl.contains("front-left")
            || pl.contains("_0")
            || pl.ends_with(":0");
        let is_fr = pl.contains("fr")
            || pl.contains("front-right")
            || pl.contains("_1")
            || pl.ends_with(":1");
        if is_fl && fl.is_none() {
            fl = Some((*p).clone());
        } else if is_fr && fr.is_none() {
            fr = Some((*p).clone());
        }
    }
    // Fallback: first two ports in order
    if fl.is_none() || fr.is_none() {
        if candidates.len() >= 2 {
            return Some((candidates[0].clone(), candidates[1].clone()));
        }
        return None;
    }
    Some((fl.unwrap(), fr.unwrap()))
}

/// Build stereo port pairs for Pulse-style `source` → `sink`.
fn port_pairs(source: &str, sink: &str) -> Result<Vec<(String, String)>> {
    // Cached listings — uncached -o/-i on every spine poll was a multi-second HOL.
    let outs = cli::pw_link_outputs()
        .ok_or_else(|| anyhow!("pw-link -o failed / empty"))?;
    let inns = cli::pw_link_inputs()
        .ok_or_else(|| anyhow!("pw-link -i failed / empty"))?;

    let src_node = pw_node(source);
    let dst_node = pw_node(sink);

    let src_ports = ports_for_node(&outs, &src_node);
    let dst_ports = ports_for_node(&inns, &dst_node);

    if src_ports.is_empty() {
        return Err(anyhow!(
            "no PW output ports for node `{src_node}` (from Pulse source `{source}`)"
        ));
    }
    if dst_ports.is_empty() {
        return Err(anyhow!(
            "no PW input ports for node `{dst_node}` (from Pulse sink `{sink}`)"
        ));
    }

    // bus.monitor → dest: monitor_* → playback_*
    // HW capture → bus: capture_* (never prefer "monitor" just because a port name matches).
    let prefer_src_monitor = is_monitor_source(source);
    let (s_fl, s_fr) = pick_lr(&src_ports, prefer_src_monitor).ok_or_else(|| {
        anyhow!("could not pick L/R outputs on `{src_node}`")
    })?;
    let (d_fl, d_fr) = pick_lr(&dst_ports, false).ok_or_else(|| {
        anyhow!("could not pick L/R inputs on `{dst_node}`")
    })?;

    Ok(vec![(s_fl, d_fl), (s_fr, d_fr)])
}

pub fn link_is_live(source: &str, sink: &str) -> bool {
    // Cold path only (caller uses this when native registry is down).
    // Pulse module presence alone is never treated as live on the hot path —
    // see `ensure_link` / `backend::link_is_live` (native-ready ⇒ native only).
    let Ok(pairs) = port_pairs(source, sink) else {
        return false;
    };
    let Some(links) = cli::pw_link_listing() else {
        return false;
    };
    // Both FL and FR must appear as connected. `pw-link -l` often uses a tree:
    //   out_port
    //    |-> in_port
    // so out and in are rarely on the same line.
    pairs.iter().all(|(o, i)| pair_linked_in_listing(&links, o, i))
}

fn pair_linked_in_listing(links: &str, out_p: &str, in_p: &str) -> bool {
    let mut current_out: Option<&str> = None;
    for line in links.lines() {
        let t = line.trim();
        if t.contains("->") && !t.starts_with("|->") && !t.starts_with("|<-") {
            let parts: Vec<_> = t.split("->").map(str::trim).collect();
            if parts.len() == 2 && parts[0] == out_p && parts[1] == in_p {
                return true;
            }
            // Substring fallback for decorated port names.
            if parts.len() == 2 && parts[0].contains(out_p) && parts[1].contains(in_p) {
                return true;
            }
            continue;
        }
        if !t.starts_with("|->") && !t.starts_with("|<-") && t.contains(':') {
            current_out = Some(t);
            continue;
        }
        if let Some(out) = current_out {
            if let Some(inp) = t.strip_prefix("|->").map(str::trim) {
                if out == out_p && inp == in_p {
                    return true;
                }
                if out.contains(out_p) && inp.contains(in_p) {
                    return true;
                }
            }
        }
    }
    false
}

/// Unload HW/`alsa_*` → `buschain_rs_*` Pulse loopbacks whose source is not in
/// any track's input rack (global allow list).
pub fn unload_orphan_hw_to_rs_loopbacks(allow_sources: &std::collections::HashSet<String>) -> u32 {
    let Ok(text) = run_capture("pactl", &["list", "short", "modules"]) else {
        return 0;
    };
    let mut n = 0u32;
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(idx) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let args = parts.next().unwrap_or("");
        if name != "module-loopback" || !args.contains("sink=buschain_rs_") {
            continue;
        }
        let src = args
            .split_whitespace()
            .find_map(|t| t.strip_prefix("source="))
            .unwrap_or("");
        // Prefix match — Pulse often uses `source=alsa_….monitor` while Desired
        // stores the node name; exact-only matching killed live Mic→rs hops.
        if src.is_empty()
            || allow_sources.iter().any(|a| {
                src == a || src.starts_with(&format!("{a}.")) || a.starts_with(&format!("{src}."))
            })
        {
            continue;
        }
        if run_status("pactl", &["unload-module", idx]).is_ok() {
            n += 1;
        }
    }
    n
}

/// Unload Pulse `module-loopback` modules that target `sink`, except those whose
/// `source=` is in `allow_sources`. Empty allow ⇒ unload every loopback into sink.
/// (Leftover loopbacks after clearing the input rack were feeding mics into tracks.)
pub fn unload_legacy_loopbacks_into_sink_except(sink: &str, allow_sources: &[String]) {
    let snk = format!("sink={sink}");
    let Ok(text) = run_capture("pactl", &["list", "short", "modules"]) else {
        return;
    };
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(idx) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let args = parts.next().unwrap_or("");
        if name != "module-loopback" || !args.contains(&snk) {
            continue;
        }
        if args.contains("sink=buschain_hold") {
            continue;
        }
        let src = args
            .split_whitespace()
            .find_map(|t| t.strip_prefix("source="))
            .unwrap_or("");
        if !src.is_empty()
            && allow_sources.iter().any(|a| {
                a == src || src.starts_with(&format!("{a}.")) || a.starts_with(&format!("{src}."))
            })
        {
            continue;
        }
        let _ = run_status("pactl", &["unload-module", idx]);
    }
}

/// Drop live PipeWire links into `sink`'s playback ports from peers that are not
/// allowed capture sources / internal mix. Keeps Desired `buschain_rs_*` peers
/// listed in `allow_bridges`; orphan rate-bridges are stripped.
pub fn unlink_capture_into_sink_except(sink: &str, allow_sources: &[String]) -> u32 {
    unlink_capture_into_sink_except_with_bridges(sink, allow_sources, &[])
}

/// Like [`unlink_capture_into_sink_except`], but keeps rate-bridge peers in
/// `allow_bridges` (node names without `.monitor`).
pub fn unlink_capture_into_sink_except_with_bridges(
    sink: &str,
    allow_sources: &[String],
    allow_bridges: &[String],
) -> u32 {
    let snk_node = pw_node(sink);
    let Ok(links) = run_capture("pw-link", &["-l"]) else {
        return 0;
    };
    let mut n = 0u32;
    let mut current_port: Option<String> = None;
    for line in links.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // Tree form: `sink:playback_FL` then `  |<- peer:output_FL`
        if let Some(rest) = t.strip_prefix("|<-") {
            let out = rest.trim();
            let Some(inn) = current_port.as_deref() else {
                continue;
            };
            if !port_on_node(inn, &snk_node) {
                continue;
            }
            if !(inn.contains(":playback_") || inn.contains(":input_")) {
                continue;
            }
            let out_node = out.split(':').next().unwrap_or(out);
            if capture_peer_allowed(out_node, allow_sources, allow_bridges) {
                continue;
            }
            // Timed — unbounded `pw-link -d` hung capture reconcile for ~30s.
            if run_status("pw-link", &["-d", out, inn]).is_ok() {
                n += 1;
            }
            continue;
        }
        // Tree form: `peer:output_FL` then `  |-> sink:playback_FL`
        if let Some(rest) = t.strip_prefix("|->") {
            let inn = rest.trim();
            let Some(out) = current_port.as_deref() else {
                continue;
            };
            if !port_on_node(inn, &snk_node) {
                continue;
            }
            if !(inn.contains(":playback_") || inn.contains(":input_")) {
                continue;
            }
            let out_node = out.split(':').next().unwrap_or(out);
            if capture_peer_allowed(out_node, allow_sources, allow_bridges) {
                continue;
            }
            if run_status("pw-link", &["-d", out, inn]).is_ok() {
                n += 1;
            }
            continue;
        }
        if t.starts_with('|') {
            continue;
        }
        if t.contains(':') {
            current_port = Some(t.to_string());
        }
    }
    if n > 0 {
        super::cli::invalidate_probe_caches();
    }
    n
}

fn capture_peer_allowed(
    peer: &str,
    allow_sources: &[String],
    allow_bridges: &[String],
) -> bool {
    // Internal mix / FX / meters — never strip as "capture".
    if peer.starts_with("buschain_track_")
        || peer.starts_with("buschain_master")
        || peer.starts_with("buschain_post_")
        || peer.starts_with("buschain_fx_")
        || peer == "buschain_hold"
        || peer.starts_with("meter-")
    {
        return true;
    }
    // Pulse loopback helper nodes — removed via unload_legacy_loopbacks_into_sink_except.
    if peer.starts_with("output.loopback") || peer.starts_with("input.loopback") {
        return false;
    }
    // Desired rate bridges stay linked; orphans are stripped.
    if peer.starts_with("buschain_rs_") || peer.starts_with("shadow_rs_") {
        return allow_bridges
            .iter()
            .any(|b| peer == b || peer.starts_with(&format!("{b}.")));
    }
    allow_sources
        .iter()
        .any(|s| peer == s || peer.starts_with(&format!("{s}.")))
}

/// True when BusChain ownership table recorded this Pulse hop.
pub fn pulse_loopback_owned(source: &str, sink: &str) -> bool {
    OWNED
        .lock()
        .map(|g| g.contains(&(source.to_string(), sink.to_string())))
        .unwrap_or(false)
}

/// Unload Pulse module-loopback for this pair only when we own it (or force).
pub fn unload_legacy_loopback(source: &str, sink: &str) {
    unload_legacy_loopback_inner(source, sink, false);
}

/// Force-unload Pulse loopback (capture force-recreate / teardown).
pub fn unload_legacy_loopback_force(source: &str, sink: &str) {
    unload_legacy_loopback_inner(source, sink, true);
}

fn unload_legacy_loopback_inner(source: &str, sink: &str, force: bool) {
    if !force && !pulse_loopback_owned(source, sink) && !pulse_loopback_exists(source, sink) {
        return;
    }
    // Prefer owned-only unload on hot path; force tears stale ghosts after mute/×.
    if !force && !pulse_loopback_owned(source, sink) {
        return;
    }
    let src = format!("source={source}");
    let snk = format!("sink={sink}");
    let Ok(out) = Command::new("pactl")
        .args(["list", "short", "modules"])
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(idx) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let args = parts.next().unwrap_or("");
        if name == "module-loopback" && args.contains(&src) && args.contains(&snk) {
            let _ = Command::new("pactl")
                .args(["unload-module", idx])
                .status();
        }
    }
    if let Ok(mut g) = OWNED.lock() {
        g.remove(&(source.to_string(), sink.to_string()));
    }
}

fn try_link(source: &str, sink: &str) -> Result<()> {
    let pairs = port_pairs(source, sink)?;
    for (out_p, in_p) in &pairs {
        // Disconnect only this exact pair (ignore errors if not linked).
        // Timed — unbounded `pw-link -d` hung ForceRespawn for ~10s.
        let _ = run_status("pw-link", &["-d", out_p.as_str(), in_p.as_str()]);
        run_status("pw-link", &[out_p.as_str(), in_p.as_str()])
            .with_context(|| format!("pw-link {out_p} → {in_p}"))?;
    }
    Ok(())
}

/// Ensure a stereo PipeWire link from Pulse `source` → `sink`.
///
/// When the native registry is ready, Pulse module presence alone is never success.
/// Pulse fallback is cold-path only (`!native_ready`) or explicit escape hatch.
pub fn ensure_link(source: &str, sink: &str) -> Result<()> {
    ensure_link_inner(source, sink, false)
}

/// Force-recreate: tear any stale hop first (Add In after mute/×).
pub fn ensure_link_force(source: &str, sink: &str) -> Result<()> {
    ensure_link_inner(source, sink, true)
}

fn ensure_link_inner(source: &str, sink: &str, force: bool) -> Result<()> {
    let native_up = super::native::native_ready();
    if force {
        unload_legacy_loopback_force(source, sink);
        let _ = unlink(source, sink);
    } else if native_up {
        // Native registry: only port-verified live skips recreate.
        if super::native::native_link_is_live(source, sink) {
            if let Ok(mut g) = OWNED.lock() {
                g.insert((source.to_string(), sink.to_string()));
            }
            return Ok(());
        }
        // Stale Pulse ghost after teardown — never Ok on module alone.
        if pulse_loopback_exists(source, sink) {
            unload_legacy_loopback_force(source, sink);
        }
    } else {
        // Cold path (registry down): Pulse or CLI listing may count as live.
        if pulse_loopback_exists(source, sink) || link_is_live(source, sink) {
            if let Ok(mut g) = OWNED.lock() {
                g.insert((source.to_string(), sink.to_string()));
            }
            return Ok(());
        }
        unload_legacy_loopback_force(source, sink);
    }

    // Two tries — 8× CLI_TIMEOUT under load made every ensure_link cost seconds.
    let mut last_err = None;
    for _ in 0..2 {
        match try_link(source, sink) {
            Ok(()) => {
                if let Ok(mut g) = OWNED.lock() {
                    g.insert((source.to_string(), sink.to_string()));
                }
                super::cli::invalidate_probe_caches();
                // Verify port links when native is up — module create ≠ live.
                if native_up && !super::native::native_link_is_live(source, sink) {
                    // try_link used CLI ports; give registry a beat.
                    std::thread::sleep(std::time::Duration::from_millis(15));
                    if !super::native::native_link_is_live(source, sink)
                        && !link_is_live(source, sink)
                    {
                        last_err = Some(anyhow!(
                            "pw-link reported ok but hop not live {source}→{sink}"
                        ));
                        continue;
                    }
                }
                return Ok(());
            }
            Err(e) => last_err = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    let dst = pw_node(sink);
    if dst.starts_with("buschain_fx_") || pw_node(source).starts_with("buschain_fx_") {
        return Err(last_err.unwrap_or_else(|| {
            anyhow!("pw-link ports not ready for host FX `{source}` → `{sink}`")
        }));
    }

    // Pulse demoted: only when registry is down, or explicit escape hatch.
    let allow_pulse = !native_up
        || matches!(
            std::env::var("BUSCHAIN_ALLOW_PULSE_CAPTURE").as_deref(),
            Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
        );
    if !allow_pulse {
        return Err(last_err.unwrap_or_else(|| {
            anyhow!("native capture hop failed {source}→{sink} (Pulse demoted)")
        }));
    }

    let pe = last_err
        .as_ref()
        .map(|e| format!("{e:#}"))
        .unwrap_or_else(|| "pw-link failed".into());
    eprintln!("[buschain] WARN: pulse module-loopback fallback {source}→{sink} ({pe})");
    match load_pulse_loopback(source, sink) {
        Ok(()) => {
            if let Ok(mut g) = OWNED.lock() {
                g.insert((source.to_string(), sink.to_string()));
            }
            Ok(())
        }
        Err(e) => Err(last_err
            .map(|pe| anyhow!("{pe}; pulse fallback: {e}"))
            .unwrap_or(e)),
    }
}

/// Wait until a sink node exposes playback ports (rate-bridge just created).
pub fn wait_sink_playback_ports(sink: &str, budget: std::time::Duration) -> bool {
    let node = pw_node(sink);
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        super::cli::invalidate_probe_caches();
        if let Some(inns) = cli::pw_link_inputs() {
            let ports = ports_for_node(&inns, &node);
            if ports.iter().any(|p| p.contains("playback") || p.contains("input")) {
                return true;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

fn load_pulse_loopback(source: &str, sink: &str) -> Result<()> {
    run_status(
        "pactl",
        &[
            "load-module",
            "module-loopback",
            &format!("source={source}"),
            &format!("sink={sink}"),
            "latency_msec=5",
            "source_dont_move=true",
            "sink_dont_move=true",
            "channels=2",
            "channel_map=front-left,front-right",
            "media.name=buschain-control",
        ],
    )?;
    for _ in 0..20 {
        if pulse_loopback_exists(source, sink) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    Err(anyhow!("module-loopback {source}→{sink} did not appear"))
}

fn pulse_loopback_exists(source: &str, sink: &str) -> bool {
    let Ok(out) = Command::new("pactl")
        .args(["list", "short", "modules"])
        .output()
    else {
        return false;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let src = format!("source={source}");
    let snk = format!("sink={sink}");
    text.lines().any(|l| {
        l.contains("module-loopback") && l.contains(&src) && l.contains(&snk)
    })
}

pub fn unlink(source: &str, sink: &str) -> Result<()> {
    unload_legacy_loopback(source, sink);
    if let Ok(pairs) = port_pairs(source, sink) {
        for (out_p, in_p) in pairs {
            let _ = Command::new("pw-link")
                .args(["-d", &out_p, &in_p])
                .output();
        }
    }
    if let Ok(mut g) = OWNED.lock() {
        g.remove(&(source.to_string(), sink.to_string()));
    }
    Ok(())
}

/// Drop links from `source` that do not target one of `allow_sinks`.
/// Never removes keepalive → `buschain_hold`.
pub fn unlink_from_source_except(source: &str, allow_sinks: &[&str]) -> u32 {
    unload_legacy_from_source_except(source, allow_sinks);

    let src_node = pw_node(source);
    let Ok(links) = run_capture("pw-link", &["-l"]) else {
        return 0;
    };
    let Ok(pairs_guess) = port_pairs(source, "buschain_hold") else {
        // Still try line-based disconnect for this source node.
        return unlink_source_lines(&links, &src_node, allow_sinks);
    };
    let _ = pairs_guess;

    unlink_source_lines(&links, &src_node, allow_sinks)
}

/// True when `port` is exactly `node` or `node:…` — not a longer sibling
/// (`buschain_fx_X_out` must not match node `buschain_fx_X`).
fn port_on_node(port: &str, node: &str) -> bool {
    port == node || port.starts_with(&format!("{node}:"))
}

fn unlink_source_lines(links: &str, src_node: &str, allow_sinks: &[&str]) -> u32 {
    let mut n = 0u32;
    // pw-link -l format varies; handle "out -> in" and tree "out" / "|-> in"
    let mut current_out: Option<String> = None;
    for line in links.lines() {
        let t = line.trim();
        if t.contains("->") && !t.starts_with("|->") && !t.starts_with("|<-") {
            let parts: Vec<_> = t.split("->").map(str::trim).collect();
            if parts.len() == 2 {
                let out_p = parts[0];
                let in_p = parts[1];
                if port_on_node(out_p, src_node) {
                    n += maybe_disconnect(out_p, in_p, allow_sinks);
                }
            }
            continue;
        }
        if !t.starts_with("|->") && !t.starts_with("|<-") && t.contains(':') {
            current_out = Some(t.to_string());
            continue;
        }
        if let Some(out_p) = &current_out {
            if !port_on_node(out_p, src_node) {
                continue;
            }
            if let Some(in_p) = t.strip_prefix("|->").map(str::trim) {
                n += maybe_disconnect(out_p, in_p, allow_sinks);
            }
        }
    }
    if let Ok(mut g) = OWNED.lock() {
        g.retain(|(s, d)| {
            pw_node(s) != *src_node || allow_sinks.iter().any(|a| d == *a) || d == "buschain_hold"
        });
    }
    n
}

fn port_targets_allowed(in_p: &str, allow_sinks: &[&str]) -> bool {
    allow_sinks.iter().any(|s| {
        // Match node prefix only — never substring (avoids keeping stale links).
        in_p == *s || in_p.starts_with(&format!("{s}:"))
    })
}

fn maybe_disconnect(out_p: &str, in_p: &str, allow_sinks: &[&str]) -> u32 {
    if in_p.starts_with("buschain_hold:") || in_p == "buschain_hold" {
        return 0;
    }
    // Pulse meter record streams attach as `meter-*:input_*` on bus monitors.
    // Never prune them — route reconcile was blanking Master/track meters.
    if in_p.starts_with("meter-") {
        return 0;
    }
    if port_targets_allowed(in_p, allow_sinks) {
        return 0;
    }
    if Command::new("pw-link")
        .args(["-d", out_p, in_p])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        1
    } else {
        0
    }
}

/// Unload Pulse `module-loopback` hops from `source` except allowed sinks / hold.
/// Used by mute disarm when native unlink skips Pulse (registry ready).
///
/// Always timed — untimeout'd `pactl list modules` on the idle mute path wedged
/// the ENGINE mutex for tens of seconds (dead meters / Master silence).
pub fn unload_legacy_from_source_except(source: &str, allow_sinks: &[&str]) {
    let src = format!("source={source}");
    let Ok(text) = super::cli::run_capture(
        "pactl",
        &["list", "short", "modules"],
        super::cli::CLI_TIMEOUT,
    ) else {
        return;
    };
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(idx) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let args = parts.next().unwrap_or("");
        if name != "module-loopback" || !args.contains(&src) {
            continue;
        }
        if args.contains("sink=buschain_hold") {
            continue;
        }
        let allowed = allow_sinks
            .iter()
            .any(|s| args.contains(&format!("sink={s}")));
        if allowed {
            continue;
        }
        let _ = super::cli::run_status(
            "pactl",
            &["unload-module", idx],
            super::cli::CLI_TIMEOUT,
        );
    }
}

pub fn teardown_buschain_links() {
    if let Ok(mut g) = OWNED.lock() {
        let pairs: Vec<_> = g.drain().collect();
        for (s, d) in pairs {
            let _ = unlink(&s, &d);
        }
    }
    let Ok(out) = Command::new("pactl")
        .args(["list", "short", "modules"])
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(idx) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let args = parts.next().unwrap_or("");
        if name != "module-loopback" {
            continue;
        }
        if args.contains("media.name=buschain-control")
            || args.contains("media.name=buschain-keepalive")
            || args.contains("media.name=shadow-audio")
            || args.contains("media.name=shadow-fx")
            || args.contains("buschain_fx_")
            || args.contains("buschain_track_")
            || args.contains("buschain_master")
            || args.contains("buschain_post_")
            || args.contains("buschain_rs_")
            || args.contains("shadow_fx_")
            || args.contains("shadow_track_")
            || args.contains("shadow_master")
            || args.contains("shadow_post_")
            || args.contains("shadow_rs_")
            || args.contains("shadow_hold")
        {
            let _ = Command::new("pactl")
                .args(["unload-module", idx])
                .status();
        }
    }
}
