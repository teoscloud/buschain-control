//! SPA Props POD builders for volume/mute and string key props.

use std::io::Cursor;

use spa::pod::serialize::PodSerializer;
use spa::pod::{Object, Property, Value, ValueArray};

/// Serialize Props with mute + stereo channelVolumes (linear gain).
pub fn pod_mute_volumes(mute: bool, linear: f32, channels: usize) -> Vec<u8> {
    let n = channels.max(1);
    let v = linear.clamp(0.0, 1.5);
    let vols = vec![v; n];
    let value = Value::Object(Object {
        type_: spa_sys::SPA_TYPE_OBJECT_Props,
        id: spa_sys::SPA_PARAM_Props,
        properties: vec![
            Property::new(spa_sys::SPA_PROP_mute, Value::Bool(mute)),
            Property::new(
                spa_sys::SPA_PROP_channelVolumes,
                Value::ValueArray(ValueArray::Float(vols)),
            ),
            Property::new(spa_sys::SPA_PROP_volume, Value::Float(v)),
        ],
    });
    PodSerializer::serialize(Cursor::new(Vec::new()), &value)
        .map(|(c, _)| c.into_inner())
        .unwrap_or_default()
}

/// Best-effort string props (device.description, audio.rate, …) as SPA Props.
/// Unknown keys are skipped — PipeWire ignores unrecognized SPA_PROP ids.
pub fn pod_string_props(entries: &[(String, String)]) -> Option<Vec<u8>> {
    let mut properties = Vec::new();
    for (k, v) in entries {
        let key = match k.as_str() {
            "device.description" | "node.description" => spa_sys::SPA_PROP_device,
            // Fall through: encode as generic device string when possible.
            _ => continue,
        };
        // SPA_PROP_device is a string path-like; for description use nick via volume path.
        // Prefer emitting mute-less description via node.description key as Float skip —
        // use SPA_PROP_device for description string (WirePlumber maps loosely).
        let _ = key;
        let _ = v;
    }
    // Encode description as a Props object with device string when present.
    if let Some((_, desc)) = entries
        .iter()
        .find(|(k, _)| k == "device.description" || k == "node.description")
    {
        properties.push(Property::new(
            spa_sys::SPA_PROP_device,
            Value::String(desc.clone()),
        ));
    }
    if properties.is_empty() {
        return None;
    }
    let value = Value::Object(Object {
        type_: spa_sys::SPA_TYPE_OBJECT_Props,
        id: spa_sys::SPA_PARAM_Props,
        properties,
    });
    PodSerializer::serialize(Cursor::new(Vec::new()), &value)
        .ok()
        .map(|(c, _)| c.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mute_volumes_pod_nonempty() {
        let b = pod_mute_volumes(true, 0.0, 2);
        assert!(!b.is_empty());
        let b2 = pod_mute_volumes(false, 1.0, 2);
        assert!(!b2.is_empty());
    }
}
