//! The `lighthouse-plugin.toml` from before the resource model.

use lighthouse_resource::API_VERSION;

/// The kind of the document a manifest migrates to.
const PLUGIN_KIND: &str = "Plugin";

/// The keys a manifest from before the resource model had.
const LEGACY_KEYS: [&str; 4] = ["id", "version", "command", "args"];

/// Whether `old` is shaped like a manifest from before the resource model: a
/// table with an `id` and a `command`.
pub fn is_legacy(old: &serde_json::Value) -> bool {
    old.get("id").is_some() && old.get("command").is_some()
}

/// A `Plugin` document from the `id`, `version`, `command` and `args` of a
/// manifest from before the resource model.
pub fn migrate(old: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::{Map, Value, json};
    let old = old.as_object().ok_or("a plugin manifest is a table")?;
    if let Some(key) = old.keys().find(|k| !LEGACY_KEYS.contains(&k.as_str())) {
        return Err(format!("unknown key `{key}`"));
    }
    let text = |key: &str| {
        old.get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("`{key}` is missing or not text"))
    };
    let mut runtime = Map::new();
    runtime.insert("command".to_owned(), json!(text("command")?));
    if let Some(args) = old
        .get("args")
        .filter(|a| a.as_array().is_some_and(|a| !a.is_empty()))
    {
        runtime.insert("args".to_owned(), args.clone());
    }
    Ok(json!({
        "apiVersion": API_VERSION,
        "kind": PLUGIN_KIND,
        "metadata": { "name": text("id")? },
        "spec": { "version": text("version")?, "runtime": runtime },
    }))
}
