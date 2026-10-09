use serde_json::{Map, Value};

use crate::expected::truncate;

/// Longest single evidence value, in characters.
const VALUE_CHARS: usize = 120;

/// The evidence without values that repeat the owner symbol, each value cut
/// to a bounded length.
pub(crate) fn evidence(evidence: &Value, symbol: Option<&str>) -> Value {
    let Some(map) = evidence.as_object() else {
        return evidence.clone();
    };
    let kept: Map<String, Value> = map
        .iter()
        .filter(|(_, value)| value.as_str().is_none_or(|s| Some(s) != symbol))
        .map(|(key, value)| (key.clone(), bounded(value)))
        .collect();
    if kept.is_empty() {
        Value::Null
    } else {
        Value::Object(kept)
    }
}

pub(crate) fn bounded(value: &Value) -> Value {
    let text = value.to_string();
    if text.chars().count() > VALUE_CHARS {
        Value::String(truncate(&text, VALUE_CHARS))
    } else {
        value.clone()
    }
}

/// `key=value` pairs of an evidence object.
pub(crate) fn evidence_line(evidence: &Value) -> Option<String> {
    pairs(evidence.as_object()?)
}

/// `key=value` pairs of a map, none when it is empty.
pub(crate) fn pairs(map: &Map<String, Value>) -> Option<String> {
    let pairs: Vec<String> = map
        .iter()
        .map(|(key, value)| format!("{key}={}", evidence_value(value)))
        .collect();
    (!pairs.is_empty()).then(|| pairs.join(" "))
}

pub(crate) fn evidence_value(value: &Value) -> String {
    match value {
        Value::String(s) if !s.contains(char::is_whitespace) && !s.is_empty() => s.clone(),
        other => other.to_string(),
    }
}
