//! The `lighthouse.toml` from before the resource model, and its YAML and
//! JSON equivalents.

use lighthouse_resource::API_VERSION;

/// The keys a `lighthouse.toml` from before the resource model had.
const LEGACY_KEYS: [&str; 5] = ["plugins", "languages", "extends", "rules", "overrides"];

/// Whether `old` is shaped like a configuration from before the resource
/// model: a table with at least one of its keys. Any other table is not
/// Lighthouse's.
pub fn is_legacy(old: &serde_json::Value) -> bool {
    old.as_object()
        .is_some_and(|table| LEGACY_KEYS.iter().any(|key| table.contains_key(*key)))
}

/// A `Project` document named `name` from a `lighthouse.toml` (or its YAML or
/// JSON equivalent) from before the resource model: `timeout` numbers become
/// durations, rule options move under `options`, `review` becomes `info` and
/// `inPlace` becomes `in-place`.
pub fn migrate(old: &serde_json::Value, name: &str) -> Result<serde_json::Value, String> {
    use serde_json::{Map, Value, json};
    let old = old.as_object().ok_or("a configuration is a table")?;
    let mut spec = Map::new();
    for (key, value) in old {
        let value = match key.as_str() {
            "plugins" => migrate_plugins(value)?,
            "languages" => migrate_languages(value),
            "rules" => migrate_rules(value)?,
            "overrides" => migrate_overrides(value)?,
            "extends" => value.clone(),
            other => return Err(format!("unknown key `{other}`")),
        };
        spec.insert(key.clone(), value);
    }
    Ok(json!({
        "apiVersion": API_VERSION,
        "kind": "Project",
        "metadata": { "name": name },
        "spec": Value::Object(spec),
    }))
}

/// Renames the options a `Project` document sets by the names they had before
/// they were camelCase, in the file as it will be written (a legacy file
/// already converted in this run, else as it is on disk), in its own format.
/// Whether the file changed; what could not be settled is added to the notes
/// of the plan.
pub(super) fn modernize_file(
    path: &std::path::Path,
    plan: &mut super::Plan,
) -> crate::Result<bool> {
    use lighthouse_resource::{Format, documents};
    let format = Format::of_path(path).unwrap_or(Format::Toml);
    let label = path.display().to_string();
    let text = match plan.actions.get(path) {
        Some(super::Action::Write(text)) => text.clone(),
        _ => std::fs::read_to_string(path)?,
    };
    let mut docs = documents(format, &label, &text)?;
    let is_project = docs.len() == 1
        && docs[0].get("apiVersion").is_some()
        && docs[0].get("kind").and_then(serde_json::Value::as_str) == Some("Project");
    if !is_project {
        return Ok(false);
    }
    let mut doc = docs.remove(0);
    let before = doc.clone();
    let mut notes = Vec::new();
    if let Some(spec) = doc.get_mut("spec") {
        let declared = super::modern::Declared::near(path);
        super::modern::rename_project_options(spec, &declared, &mut notes);
    }
    plan.kept
        .extend(notes.into_iter().map(|n| format!("{label}: {n}")));
    if doc == before {
        return Ok(false);
    }
    let text = super::render_config(format, path, &doc)?;
    plan.actions
        .insert(path.to_owned(), super::Action::Write(text));
    Ok(true)
}

fn migrate_plugins(value: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    let list = value.as_array().ok_or("`plugins` is a list")?;
    let migrated = list.iter().map(|plugin| match plugin {
        Value::Object(table) => {
            let mut table = table.clone();
            if let Some(Value::Number(seconds)) = table.get("timeout") {
                let text = format!("{seconds}s");
                table.insert("timeout".to_owned(), Value::String(text));
            }
            Value::Object(table)
        }
        other => other.clone(),
    });
    Ok(Value::Array(migrated.collect()))
}

fn migrate_languages(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    let Value::Object(languages) = value else {
        return value.clone();
    };
    let mut out = languages.clone();
    for language in out.values_mut() {
        let Some(Value::Object(table)) = language.get_mut("formatter") else {
            continue;
        };
        if table.get("output").and_then(Value::as_str) == Some("inPlace") {
            table.insert("output".to_owned(), Value::String("in-place".to_owned()));
        }
    }
    Value::Object(out)
}

fn migrate_rules(value: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::{Map, Value};
    let rules = value.as_object().ok_or("`rules` is a table")?;
    let mut out = Map::new();
    for (id, rule) in rules {
        let migrated = match rule {
            Value::String(level) => Value::String(migrate_level(level)),
            Value::Object(table) => {
                let mut table = table.clone();
                let level = match table.remove("level") {
                    Some(Value::String(level)) => migrate_level(&level),
                    _ => return Err(format!("rule `{id}` is a table without a string `level`")),
                };
                let mut detail = Map::new();
                detail.insert("level".to_owned(), Value::String(level));
                if !table.is_empty() {
                    detail.insert("options".to_owned(), Value::Object(table));
                }
                Value::Object(detail)
            }
            _ => return Err(format!("rule `{id}` is a level string or a table")),
        };
        out.insert(id.clone(), migrated);
    }
    Ok(Value::Object(out))
}

fn migrate_overrides(value: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    let list = value.as_array().ok_or("`overrides` is a list")?;
    let mut out = Vec::new();
    for item in list {
        let mut item = item.as_object().ok_or("an override is a table")?.clone();
        if let Some(rules) = item.get("rules") {
            let migrated = migrate_rules(rules)?;
            item.insert("rules".to_owned(), migrated);
        }
        out.push(Value::Object(item));
    }
    Ok(Value::Array(out))
}

/// `review` was a level; it is `info` now.
fn migrate_level(level: &str) -> String {
    if level == "review" { "info" } else { level }.to_owned()
}
