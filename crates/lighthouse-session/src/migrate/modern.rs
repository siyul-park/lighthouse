//! Revision 28 of the decision model: the fields that duplicated others are
//! gone and option keys are camelCase. A `Decision` document that still has
//! them, and a `Project` whose rules set options by their old names, are
//! rewritten; running it again changes nothing.
//!
//! - `intent` (and `rationale`) become `context`, and a language's `tuning`
//!   is appended to it under the language's name;
//! - `exceptions` are appended to the `requirement`; the appended text is
//!   kept in the annotation `lighthouse/was-exceptions`, so verdicts recorded
//!   under the requirement without it keep applying;
//! - `citation` becomes `provenance.wasDerivedFrom`;
//! - the declared `evidence` is dropped (the check says what it emits);
//! - `strict: true` becomes the label `lighthouse/preset: strict`;
//! - a `cel` `select` that repeats the decision's `scope.subject` is dropped;
//! - option names go from snake_case to camelCase, in the declaration, the
//!   languages, the examples and the expressions that read them.

use std::collections::BTreeSet;

use lighthouse_spec::{Catalog, PRESET_LABEL, STRICT, WAS_EXCEPTIONS};
use serde_json::Value as Json;
use serde_norway::{Mapping, Value};

/// The spec keys that no longer exist.
const RETIRED: [&str; 6] = [
    "intent",
    "rationale",
    "exceptions",
    "citation",
    "evidence",
    "strict",
];

/// Whether a `Decision` document has anything the migration rewrites.
pub fn is_outdated(doc: &Value) -> bool {
    if doc.get("kind").and_then(Value::as_str) != Some("Decision") {
        return false;
    }
    let Some(spec) = doc.get("spec").and_then(Value::as_mapping) else {
        return false;
    };
    RETIRED.iter().any(|key| spec.contains_key(*key))
        || spec
            .get("languages")
            .and_then(Value::as_mapping)
            .is_some_and(|languages| languages.values().any(|l| l.get("tuning").is_some()))
        || !snake_options(spec).is_empty()
        || repeats_subject(spec)
}

/// Rewrites a `Decision` document in place.
pub fn modernize(doc: &mut Value) {
    let Some(root) = doc.as_mapping_mut() else {
        return;
    };
    let Some(Value::Mapping(mut spec)) = root.remove("spec") else {
        return;
    };
    let names = snake_options(&spec);
    let mut labels = Vec::new();
    let mut annotations = Vec::new();
    context(&mut spec);
    if let Some(Value::String(exceptions)) = spec.remove("exceptions") {
        let appended = format!(" {}", squash(&exceptions));
        if let Some(Value::String(requirement)) = spec.get_mut("requirement") {
            *requirement = format!("{}{appended}", requirement.trim_end());
        }
        annotations.push((WAS_EXCEPTIONS, appended));
    }
    if let Some(Value::String(citation)) = spec.remove("citation") {
        let mut provenance = Mapping::new();
        provenance.insert(
            "wasDerivedFrom".into(),
            Value::Sequence(vec![Value::String(citation)]),
        );
        spec.insert("provenance".into(), Value::Mapping(provenance));
    }
    spec.remove("evidence");
    if spec.remove("strict") == Some(Value::Bool(true)) {
        labels.push((PRESET_LABEL, STRICT.to_owned()));
    }
    if repeats_subject(&spec)
        && let Some(Value::Mapping(check)) = spec.get_mut("check")
    {
        check.remove("select");
    }
    for key in ["options", "languages", "check", "fix", "examples"] {
        if let Some(value) = spec.get_mut(key) {
            rename(value, &names, key == "options");
        }
    }
    root.insert("spec".into(), Value::Mapping(spec));
    add(root, "labels", labels);
    add(root, "annotations", annotations);
}

/// `hub_fan_in` as `hubFanIn`.
pub fn camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// The options of `rules` of a project by the names they have now: those a
/// bundled decision declares in camelCase are renamed from snake_case.
pub fn rename_rule_options(rules: &mut serde_json::Map<String, Json>) {
    let catalog = Catalog::bundled();
    for (id, rule) in rules.iter_mut() {
        let Some(options) = rule.get_mut("options").and_then(Json::as_object_mut) else {
            continue;
        };
        let Some(declared) = catalog
            .decision(id)
            .and_then(|d| d.options.as_ref())
            .map(|o| &o.properties)
        else {
            continue;
        };
        let renamed: serde_json::Map<String, Json> = std::mem::take(options)
            .into_iter()
            .map(|(key, v)| {
                let new = camel(&key);
                (
                    if declared.contains_key(&new) {
                        new
                    } else {
                        key
                    },
                    v,
                )
            })
            .collect();
        *options = renamed;
    }
}

/// Renames the options of every rule of a project's `spec`: its own `rules`
/// and those of its `overrides`.
pub fn rename_project_options(spec: &mut Json) {
    let Some(spec) = spec.as_object_mut() else {
        return;
    };
    if let Some(Json::Object(rules)) = spec.get_mut("rules") {
        rename_rule_options(rules);
    }
    if let Some(Json::Array(overrides)) = spec.get_mut("overrides") {
        for item in overrides {
            if let Some(Json::Object(rules)) = item.get_mut("rules") {
                rename_rule_options(rules);
            }
        }
    }
}

/// Whether a project's rules set options by names a bundled decision spells
/// differently now.
pub fn has_snake_rule_options(spec: &Json) -> bool {
    let mut probe = spec.clone();
    rename_project_options(&mut probe);
    probe != *spec
}

/// The names of the options of a decision that still have an underscore.
fn snake_options(spec: &Mapping) -> BTreeSet<String> {
    spec.get("options")
        .and_then(|o| o.get("properties"))
        .and_then(Value::as_mapping)
        .into_iter()
        .flatten()
        .filter_map(|(name, _)| name.as_str())
        .filter(|name| name.contains('_'))
        .map(str::to_owned)
        .collect()
}

fn repeats_subject(spec: &Mapping) -> bool {
    let subject = spec
        .get("scope")
        .and_then(|s| s.get("subject"))
        .and_then(Value::as_str);
    let check = spec.get("check");
    let kind = check.and_then(|c| c.get("type")).and_then(Value::as_str);
    let select = check.and_then(|c| c.get("select")).and_then(Value::as_str);
    kind == Some("cel") && select.is_some() && select == subject
}

/// `intent` and `rationale` as one `context`, with the wording each language
/// gave the decision (`tuning`) after them.
fn context(spec: &mut Mapping) {
    let mut paragraphs = Vec::new();
    for key in ["intent", "rationale"] {
        if let Some(Value::String(text)) = spec.remove(key) {
            paragraphs.push(text.trim().to_owned());
        }
    }
    if let Some(Value::Mapping(languages)) = spec.get_mut("languages") {
        for (language, value) in languages.iter_mut() {
            let Some(Value::String(tuning)) =
                value.as_mapping_mut().and_then(|l| l.remove("tuning"))
            else {
                continue;
            };
            let name = language_name(language.as_str().unwrap_or_default());
            paragraphs.push(format!("{name}: {}", squash(&tuning)));
        }
        languages.retain(|_, l| l.as_mapping().is_none_or(|m| !m.is_empty()));
    }
    if spec
        .get("languages")
        .and_then(Value::as_mapping)
        .is_some_and(Mapping::is_empty)
    {
        spec.remove("languages");
    }
    if paragraphs.is_empty() {
        return;
    }
    // The context goes where the intent was: first, after the title.
    let rest = std::mem::take(spec);
    for (key, value) in rest {
        spec.insert(key.clone(), value);
        if key.as_str() == Some("title") {
            spec.insert("context".into(), Value::String(paragraphs.join("\n\n")));
        }
    }
}

fn language_name(id: &str) -> String {
    match id {
        "go" => "Go".to_owned(),
        "rust" => "Rust".to_owned(),
        "typescript" => "TypeScript".to_owned(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
        }
    }
}

/// Renames option names in `value`: map keys when `keys` (the declaration and
/// the `options` of languages and examples hold names as keys), and the names
/// the expressions of the text read as `options.name`.
fn rename(value: &mut Value, names: &BTreeSet<String>, keys: bool) {
    match value {
        Value::String(text) => {
            for name in names {
                *text = text.replace(
                    &format!("options.{name}"),
                    &format!("options.{}", camel(name)),
                );
                *text = text.replace(&format!("`{name}`"), &format!("`{}`", camel(name)));
            }
        }
        Value::Sequence(items) => items.iter_mut().for_each(|v| rename(v, names, false)),
        Value::Mapping(map) => {
            let old = std::mem::take(map);
            for (key, mut v) in old {
                let holds_names = matches!(key.as_str(), Some("options" | "properties"));
                rename(&mut v, names, holds_names);
                let key = match key.as_str() {
                    Some(k) if keys && names.contains(k) => Value::String(camel(k)),
                    _ => key,
                };
                map.insert(key, v);
            }
        }
        _ => {}
    }
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Adds `entries` to the metadata map `field` of the document.
fn add(root: &mut Mapping, field: &str, entries: Vec<(&str, String)>) {
    if entries.is_empty() {
        return;
    }
    let metadata = root
        .entry("metadata".into())
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    let Some(metadata) = metadata.as_mapping_mut() else {
        return;
    };
    let map = metadata
        .entry(field.into())
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    if let Some(map) = map.as_mapping_mut() {
        for (key, value) in entries {
            map.insert(key.into(), Value::String(value));
        }
    }
}
