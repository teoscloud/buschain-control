//! Graph clock-master policy — Master HW must win PipeWire driver election.
//!
//! PipeWire clocks the whole graph off a single driver node: the running node
//! with the highest `priority.driver`. WirePlumber's defaults give capture nodes
//! +1000 over playback, so the moment a track input (e.g. a full-speed USB codec)
//! is adopted it out-ranks Master HW and becomes the clock for everything —
//! Master HW included. Every xrun on that device then surfaces as a click on the
//! speakers, and a global `clock.force-rate` the driver cannot run natively makes
//! it resample its own clock source (periodic corrections ≈ periodic glitches).
//!
//! `priority.driver` is a node *property*, not a SPA param: a client cannot change
//! it on a live node (verified on PipeWire 1.6 — `Props.params` is ignored). The
//! only lever is a WirePlumber rule applied when the node is (re)created, so this
//! module (1) generates a session-scoped drop-in that ranks Master HW above every
//! adopted input and (2) inspects the live election so the engine can warn when
//! Master HW is not the clock.

use std::collections::BTreeSet;
use std::path::PathBuf;

use super::{find_alsa_card_in_dump, json_u32, pw_dump_json};

/// Drop-in file name (sorted after the shipped `51-buschain-seal-helpers.conf`).
pub const RULE_FILE: &str = "53-buschain-master-hw-clock.conf";
/// Above WirePlumber's USB capture default (2109) with room for user overrides.
pub const MASTER_HW_DRIVER_PRIORITY: i64 = 5000;
/// Below every playback default (HDMI ≈ 700) — inputs only drive when alone.
pub const INPUT_DRIVER_PRIORITY: i64 = 100;

/// The node currently clocking the graph for a given follower.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphDriver {
    pub id: u32,
    pub name: String,
    pub priority: i64,
    pub card_name: Option<String>,
    /// Rates the driver advertises in `EnumFormat` (empty when unknown).
    pub native_rates: Vec<u32>,
}

impl GraphDriver {
    /// True when the driver advertises rates and `rate` is not one of them.
    pub fn cannot_run_natively(&self, rate: u32) -> bool {
        !self.native_rates.is_empty() && !self.native_rates.contains(&rate)
    }
}

/// Outcome of inspecting who clocks Master HW.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockOwner {
    /// Master HW (or a sibling node on the same ALSA card, e.g. the UCM split
    /// parent `alsa_output.hw_USB_0`) is the graph driver.
    MasterHw(GraphDriver),
    /// Another device clocks the graph — Master HW is a resampled follower.
    Foreign(GraphDriver),
    /// Master HW is not running, not found, or `pw-dump` unavailable.
    Unknown,
}

fn props_of<'a>(o: &'a serde_json::Value) -> Option<&'a serde_json::Value> {
    o.pointer("/info/props")
}

fn node_by_name<'a>(dump: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    dump.as_array()?.iter().find(|o| {
        props_of(o)
            .and_then(|p| p.get("node.name"))
            .and_then(|v| v.as_str())
            == Some(name)
    })
}

fn node_by_id<'a>(dump: &'a serde_json::Value, id: u32) -> Option<&'a serde_json::Value> {
    dump.as_array()?
        .iter()
        .find(|o| o.get("id").and_then(json_u32) == Some(id) && props_of(o).is_some())
}

fn prop_str(o: &serde_json::Value, key: &str) -> Option<String> {
    props_of(o)?.get(key)?.as_str().map(str::to_string)
}

fn prop_i64(o: &serde_json::Value, key: &str) -> Option<i64> {
    let v = props_of(o)?.get(key)?;
    v.as_i64().or_else(|| v.as_str()?.parse().ok())
}

fn node_state(o: &serde_json::Value) -> Option<&str> {
    o.pointer("/info/state")?.as_str()
}

fn enum_rates(o: &serde_json::Value) -> Vec<u32> {
    let mut set = BTreeSet::new();
    super::extract_enum_rates(o.pointer("/info/params"), &mut set);
    set.into_iter().collect()
}

fn card_name_in(dump: &serde_json::Value, node: &str) -> Option<String> {
    let node = node.strip_suffix(".monitor").unwrap_or(node);
    let o = node_by_name(dump, node)?;
    prop_str(o, "api.alsa.card.name").or_else(|| prop_str(o, "alsa.card_name"))
}

/// `api.alsa.card.name` of a node (identical on every node of one card,
/// including UCM split parents/children) — the key the generated rule matches on.
pub fn alsa_card_name(node: &str) -> Option<String> {
    card_name_in(&pw_dump_json()?, node)
}

/// Who clocks `master_hw` right now.
pub fn graph_clock_owner(master_hw: &str) -> ClockOwner {
    let hw = master_hw.strip_suffix(".monitor").unwrap_or(master_hw);
    if hw.is_empty() {
        return ClockOwner::Unknown;
    }
    let Some(dump) = pw_dump_json() else {
        return ClockOwner::Unknown;
    };
    let Some(hw_node) = node_by_name(&dump, hw) else {
        return ClockOwner::Unknown;
    };
    // A suspended/idle Master HW has no meaningful driver — only judge a live one.
    if node_state(hw_node) != Some("running") {
        return ClockOwner::Unknown;
    }
    let Some(driver_id) = prop_i64(hw_node, "node.driver-id").filter(|&id| id > 0) else {
        return ClockOwner::Unknown;
    };
    let Some(drv) = node_by_id(&dump, driver_id as u32) else {
        return ClockOwner::Unknown;
    };
    let Some(drv_name) = prop_str(drv, "node.name") else {
        return ClockOwner::Unknown;
    };
    let driver = GraphDriver {
        id: driver_id as u32,
        name: drv_name.clone(),
        priority: prop_i64(drv, "priority.driver").unwrap_or(0),
        card_name: prop_str(drv, "api.alsa.card.name"),
        native_rates: enum_rates(drv),
    };

    let hw_id = hw_node.get("id").and_then(json_u32);
    if hw_id == Some(driver.id) {
        return ClockOwner::MasterHw(driver);
    }
    // UCM split children (`HiFi__Line1__sink`) are clocked by their hw parent.
    if prop_str(hw_node, "api.alsa.split.name").as_deref() == Some(drv_name.as_str()) {
        return ClockOwner::MasterHw(driver);
    }
    // Same ALSA card (input/output of one interface share the hardware clock).
    let hw_card = find_alsa_card_in_dump(&dump, hw);
    let drv_card = find_alsa_card_in_dump(&dump, &drv_name);
    if hw_card.is_some() && hw_card == drv_card {
        return ClockOwner::MasterHw(driver);
    }
    let hw_card_name = prop_str(hw_node, "api.alsa.card.name");
    if hw_card_name.is_some() && hw_card_name == driver.card_name {
        return ClockOwner::MasterHw(driver);
    }
    ClockOwner::Foreign(driver)
}

/// Session-scoped election policy rendered into a WirePlumber drop-in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClockMasterPolicy {
    /// `api.alsa.card.name` of the Master HW sink — its playback nodes win.
    pub master_hw_card: Option<String>,
    /// `api.alsa.card.name` of every adopted `alsa_input.*` — their capture nodes lose.
    pub input_cards: BTreeSet<String>,
}

impl ClockMasterPolicy {
    /// Resolve card names from the live graph for Master HW + adopted sources.
    /// `None` when `pw-dump` is unavailable (caller must not touch the rule file).
    pub fn from_graph<'a>(
        master_hw: &str,
        sources: impl IntoIterator<Item = &'a str>,
    ) -> Option<Self> {
        let dump = pw_dump_json()?;
        let master_hw_card = if master_hw.is_empty() {
            None
        } else {
            card_name_in(&dump, master_hw)
        };
        let mut input_cards = BTreeSet::new();
        for src in sources {
            let node = src.strip_suffix(".monitor").unwrap_or(src);
            if !node.starts_with("alsa_input.") {
                continue;
            }
            if let Some(card) = card_name_in(&dump, node) {
                input_cards.insert(card);
            }
        }
        Some(Self {
            master_hw_card,
            input_cards,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.master_hw_card.is_none() && self.input_cards.is_empty()
    }

    /// WirePlumber 0.5 SPA-JSON drop-in body.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "# BusChain Control — generated from the active session; do not edit.\n\
             # Master HW must win PipeWire driver election so a track input (e.g. a\n\
             # full-speed USB codec) cannot become the graph clock and leak its xruns\n\
             # into everything that follows it. Rules apply when nodes are created:\n\
             #   systemctl --user restart wireplumber   (or re-plug the device)\n\
             monitor.alsa.rules = [\n",
        );
        if let Some(card) = &self.master_hw_card {
            out.push_str(&format!(
                "  {{\n    matches = [\n      {{ api.alsa.card.name = \"{}\"  node.name = \"~alsa_output.*\" }}\n    ]\n    actions = {{ update-props = {{ priority.driver = {} }} }}\n  }}\n",
                escape(card),
                MASTER_HW_DRIVER_PRIORITY
            ));
        }
        for card in &self.input_cards {
            out.push_str(&format!(
                "  {{\n    matches = [\n      {{ api.alsa.card.name = \"{}\"  node.name = \"~alsa_input.*\" }}\n    ]\n    actions = {{ update-props = {{ priority.driver = {} }} }}\n  }}\n",
                escape(card),
                INPUT_DRIVER_PRIORITY
            ));
        }
        out.push_str("]\n");
        out
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `$XDG_CONFIG_HOME/wireplumber/wireplumber.conf.d/53-buschain-master-hw-clock.conf`.
pub fn rule_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(
        base.join("wireplumber")
            .join("wireplumber.conf.d")
            .join(RULE_FILE),
    )
}

/// Write the drop-in when its content changed. `Ok(true)` when (re)written.
/// An empty policy removes a previously generated file.
pub fn write_rule(policy: &ClockMasterPolicy) -> Result<bool, String> {
    let Some(path) = rule_path() else {
        return Err("no XDG_CONFIG_HOME/HOME for WirePlumber drop-in".into());
    };
    if policy.is_empty() {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(format!("remove {}: {e}", path.display())),
        };
    }
    let body = policy.render();
    if std::fs::read_to_string(&path).ok().as_deref() == Some(body.as_str()) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("conf.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {e}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_ranks_master_outputs_above_input_cards() {
        let mut p = ClockMasterPolicy {
            master_hw_card: Some("Scarlett 2i4 USB".into()),
            input_cards: BTreeSet::new(),
        };
        p.input_cards.insert("USB Audio CODEC".into());
        p.input_cards.insert("Scarlett 2i4 USB".into());
        let body = p.render();
        assert!(body.starts_with('#'));
        assert!(body.contains("monitor.alsa.rules = ["));
        assert!(body.contains(
            "{ api.alsa.card.name = \"Scarlett 2i4 USB\"  node.name = \"~alsa_output.*\" }"
        ));
        assert!(body.contains("priority.driver = 5000"));
        assert!(body.contains(
            "{ api.alsa.card.name = \"USB Audio CODEC\"  node.name = \"~alsa_input.*\" }"
        ));
        assert!(body.contains(
            "{ api.alsa.card.name = \"Scarlett 2i4 USB\"  node.name = \"~alsa_input.*\" }"
        ));
        assert_eq!(body.matches("priority.driver = 100").count(), 2);
        assert!(body.trim_end().ends_with(']'));
    }

    #[test]
    fn render_escapes_quotes_in_card_names() {
        let p = ClockMasterPolicy {
            master_hw_card: Some("Card \"Pro\"".into()),
            input_cards: BTreeSet::new(),
        };
        assert!(p.render().contains("api.alsa.card.name = \"Card \\\"Pro\\\"\""));
    }

    #[test]
    fn empty_policy_has_no_rules() {
        let p = ClockMasterPolicy::default();
        assert!(p.is_empty());
        let body = p.render();
        assert!(body.contains("monitor.alsa.rules = [\n]"));
    }

    #[test]
    fn cannot_run_natively_only_when_rates_known() {
        let mut d = GraphDriver {
            id: 1,
            name: "x".into(),
            priority: 0,
            card_name: None,
            native_rates: vec![],
        };
        assert!(!d.cannot_run_natively(96_000));
        d.native_rates = vec![44_100, 48_000];
        assert!(d.cannot_run_natively(96_000));
        assert!(!d.cannot_run_natively(48_000));
    }
}
