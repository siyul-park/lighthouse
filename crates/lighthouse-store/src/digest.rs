use serde_json::Value;
use sha2::{Digest, Sha256};

/// The SHA-256 of `text` in hex, cut to `bytes` bytes.
pub(crate) fn hash(text: &str, bytes: usize) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest[..bytes].iter().map(|b| format!("{b:02x}")).collect()
}

/// A stable text for a JSON value: compact, keys in sorted order at every
/// level, so equal values give equal bytes whatever built them.
pub(crate) fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let entries: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical(&map[k])))
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", items.join(","))
        }
        other => other.to_string(),
    }
}

/// A hash of evidence with whitespace in its strings collapsed, so
/// reformatting does not change what the evidence says.
pub(crate) fn evidence(evidence: &Value) -> String {
    hash(&canonical(&normalize(evidence)), 8)
}

fn normalize(value: &Value) -> Value {
    match value {
        Value::String(s) => Value::String(s.split_whitespace().collect::<Vec<_>>().join(" ")),
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), normalize(v))).collect())
        }
        other => other.clone(),
    }
}
