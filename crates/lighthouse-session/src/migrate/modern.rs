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
//!   languages, the examples and the expressions that read them; the names
//!   they had are kept in the annotation `lighthouse/was-option-names`, so
//!   the meaning version they were hashed under can be told again.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use lighthouse_spec::{
    Catalog, FILE_NAMES, PRESET_LABEL, STRICT, WAS_EXCEPTIONS, WAS_OPTION_NAMES, local_files,
};
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

/// The options each decision of a project declares, by the names they have
/// or will have once migrated: the bundled catalog, and the decisions in the
/// project's `.lighthouse/decisions`, read as they are (they may not have
/// been migrated yet). The decisions of external plugins are not known here.
pub struct Declared(BTreeMap<String, BTreeSet<String>>);

impl Declared {
    /// What the bundled catalog declares, and the local decisions of the
    /// project whose files lie around `path`, if any.
    pub fn near(path: &Path) -> Self {
        let mut declared = BTreeMap::new();
        for decision in Catalog::bundled().decisions() {
            let names = decision
                .options
                .iter()
                .flat_map(|o| o.properties.keys().map(|n| camel(n)))
                .collect();
            declared.insert(decision.id().to_owned(), names);
        }
        if let Some(root) = project_root(path) {
            declared.extend(local_decisions(&root));
        }
        Self(declared)
    }

    fn of(&self, id: &str) -> Option<&BTreeSet<String>> {
        self.0.get(id)
    }
}

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
    if !names.is_empty() {
        annotations.push((WAS_OPTION_NAMES, was_option_names(&spec, &names)));
    }
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

/// The options of `rules` of a project by the names they have now: a name a
/// decision declares in camelCase is renamed from its snake_case spelling.
/// What cannot be settled is added to `notes`: both spellings set at once
/// (the camelCase one is kept), or options of a decision nothing here declares
/// (left as they are).
pub fn rename_rule_options(
    rules: &mut serde_json::Map<String, Json>,
    declared: &Declared,
    notes: &mut Vec<String>,
) {
    for (id, rule) in rules.iter_mut() {
        let Some(options) = rule.get_mut("options").and_then(Json::as_object_mut) else {
            continue;
        };
        let Some(names) = declared.of(id) else {
            if options.keys().any(|k| k.contains('_')) {
                notes.push(format!(
                    "rule `{id}`: its options were not renamed, no decision of that id is known here; rename snake_case options to camelCase by hand"
                ));
            }
            continue;
        };
        let target = |key: &str| {
            let new = camel(key);
            if new != key && names.contains(&new) {
                new
            } else {
                key.to_owned()
            }
        };
        let old = std::mem::take(options);
        let (kept, renamed): (Vec<_>, Vec<_>) =
            old.into_iter().partition(|(key, _)| target(key) == *key);
        options.extend(kept);
        for (key, value) in renamed {
            let new = target(&key);
            if options.contains_key(&new) {
                notes.push(format!(
                    "rule `{id}`: sets both `{key}` and `{new}`; kept `{new}`"
                ));
            } else {
                options.insert(new, value);
            }
        }
    }
}

/// Renames the options of every rule of a project's `spec`: its own `rules`
/// and those of its `overrides`.
pub fn rename_project_options(spec: &mut Json, declared: &Declared, notes: &mut Vec<String>) {
    let Some(spec) = spec.as_object_mut() else {
        return;
    };
    if let Some(Json::Object(rules)) = spec.get_mut("rules") {
        rename_rule_options(rules, declared, notes);
    }
    if let Some(Json::Array(overrides)) = spec.get_mut("overrides") {
        for item in overrides {
            if let Some(Json::Object(rules)) = item.get_mut("rules") {
                rename_rule_options(rules, declared, notes);
            }
        }
    }
}

/// The directory around `path` that holds a configuration or `.lighthouse`.
fn project_root(path: &Path) -> Option<std::path::PathBuf> {
    let absolute = path
        .canonicalize()
        .ok()
        .or_else(|| std::env::current_dir().ok().map(|dir| dir.join(path)))?;
    absolute
        .ancestors()
        .skip(usize::from(absolute.is_file()))
        .find(|dir| {
            lighthouse_resource::file_in(dir, &FILE_NAMES).is_some()
                || dir.join(".lighthouse").is_dir()
        })
        .map(Path::to_owned)
}

/// The options declared by the `Decision` documents of a project's local
/// layer, whatever shape they are in.
fn local_decisions(root: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let Ok(Some(files)) = local_files(root) else {
        return BTreeMap::new();
    };
    let mut found = BTreeMap::new();
    for (name, text) in files {
        let Ok(docs) = lighthouse_resource::yaml_values(&name, &text) else {
            continue;
        };
        for doc in docs {
            let Some(id) = doc
                .get("metadata")
                .and_then(|m| m.get("name"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let names = doc
                .get("spec")
                .and_then(|s| s.get("options"))
                .and_then(|o| o.get("properties"))
                .and_then(Value::as_mapping)
                .into_iter()
                .flatten()
                .filter_map(|(n, _)| n.as_str().map(camel))
                .collect();
            found.insert(id.to_owned(), names);
        }
    }
    found
}

/// The options of a decision that are named with an underscore and can be
/// renamed: by their old name, the new one. A name whose camelCase spelling
/// is taken stays as it is.
fn snake_options(spec: &Mapping) -> BTreeMap<String, String> {
    let all: BTreeSet<&str> = spec
        .get("options")
        .and_then(|o| o.get("properties"))
        .and_then(Value::as_mapping)
        .into_iter()
        .flatten()
        .filter_map(|(name, _)| name.as_str())
        .collect();
    let mut renamed: BTreeMap<String, String> = BTreeMap::new();
    for name in all.iter().filter(|n| n.contains('_')) {
        let new = camel(name);
        let taken = all.contains(new.as_str()) || renamed.values().any(|n| *n == new);
        if new != *name && !new.is_empty() && !taken {
            renamed.insert((*name).to_owned(), new);
        }
    }
    renamed
}

/// A JSON map from each option's new name to the one it had.
fn was_option_names(spec: &Mapping, renamed: &BTreeMap<String, String>) -> String {
    let names: BTreeMap<String, String> = spec
        .get("options")
        .and_then(|o| o.get("properties"))
        .and_then(Value::as_mapping)
        .into_iter()
        .flatten()
        .filter_map(|(name, _)| name.as_str())
        .map(|old| {
            (
                renamed.get(old).map_or(old, String::as_str).to_owned(),
                old.to_owned(),
            )
        })
        .collect();
    serde_json::to_string(&names).unwrap_or_default()
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
/// the expressions of the text read as `options.name` or `options["name"]`.
fn rename(value: &mut Value, names: &BTreeMap<String, String>, keys: bool) {
    match value {
        Value::String(text) => {
            for (old, new) in names {
                *text = replace_read(text, &format!("options.{old}"), &format!("options.{new}"));
                for quote in ['"', '\''] {
                    *text = text.replace(
                        &format!("options[{quote}{old}{quote}]"),
                        &format!("options[{quote}{new}{quote}]"),
                    );
                }
                *text = text.replace(&format!("`{old}`"), &format!("`{new}`"));
            }
        }
        Value::Sequence(items) => items.iter_mut().for_each(|v| rename(v, names, false)),
        Value::Mapping(map) => {
            let old = std::mem::take(map);
            for (key, mut v) in old {
                let holds_names = matches!(key.as_str(), Some("options" | "properties"));
                rename(&mut v, names, holds_names);
                let key = match key.as_str().and_then(|k| names.get(k)) {
                    Some(new) if keys => Value::String(new.clone()),
                    _ => key,
                };
                map.insert(key, v);
            }
        }
        _ => {}
    }
}

/// `text` with `from` replaced by `to` where `from` ends at an identifier
/// boundary: `options.a_b` is not a prefix of `options.a_b_c`.
fn replace_read(text: &str, from: &str, to: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(from) {
        let after = &rest[at + from.len()..];
        let continues = after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        out.push_str(&rest[..at]);
        out.push_str(if continues { from } else { to });
        rest = after;
    }
    out.push_str(rest);
    out
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
