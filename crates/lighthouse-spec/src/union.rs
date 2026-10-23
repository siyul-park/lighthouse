//! The unions a decision holds, a `check` and a `fix`: one provider selected
//! by `type`, with fields every provider shares. Which other fields a table
//! may have depends on its `type`, and any other is refused, by the loader
//! and by the schema alike.

use serde_json::{Map, Value};

/// Why `table`, a `noun` of some `type`, has a field that is neither
/// `common` nor in `own`.
pub(crate) fn stray_field(
    table: &Map<String, Value>,
    noun: &str,
    common: &[&str],
    own: &[&str],
) -> Option<String> {
    let stray = table
        .keys()
        .find(|key| !common.contains(&key.as_str()) && !own.contains(&key.as_str()))?;
    let kind = table["type"].as_str().unwrap_or_default();
    Some(format!(
        "unknown field `{stray}` for a {noun} of type `{kind}`"
    ))
}

/// The schema of a union as the loader reads it: each branch carries the
/// common fields too and allows no other.
pub(crate) fn refuse_strays(schema: &mut schemars::Schema) {
    let Some(root) = schema.as_object_mut() else {
        return;
    };
    let common = match root.get("properties") {
        Some(Value::Object(properties)) => properties.clone(),
        _ => Map::new(),
    };
    let required = root
        .get("required")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let Some(Value::Array(branches)) = root.get_mut("oneOf") else {
        return;
    };
    for branch in branches.iter_mut().filter_map(Value::as_object_mut) {
        if let Value::Object(properties) = branch
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()))
        {
            properties.extend(common.clone());
        }
        if let (Some(Value::Array(own)), Value::Array(shared)) =
            (branch.get_mut("required"), &required)
        {
            own.extend(shared.iter().cloned());
        }
        // A branch that refers to another schema for its own fields cannot
        // close itself with `additionalProperties`,
        // which only sees the properties written beside it.
        let closing = if branch.contains_key("$ref") {
            "unevaluatedProperties"
        } else {
            "additionalProperties"
        };
        branch.insert(closing.to_owned(), Value::Bool(false));
    }
}
