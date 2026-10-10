//! Rewrites the catalog formats from before the resource model (pattern files,
//! `pack.yaml` with `section.yaml`, local rule files, `sources.yaml`) as
//! `lighthouse/v1alpha1` documents. The converters read YAML values with the
//! keys in the order they were written and write them in the order of the new
//! schema, so a migrated file stays readable; running one on a document that
//! is already migrated is the caller's `apiVersion` check.

use std::collections::BTreeMap;

use lighthouse_resource::API_VERSION;
use serde_norway::{Mapping, Value};

use lighthouse_spec::{
    MIGRATED_FROM, PACK_LABEL, SECTION_LABEL, WAS_ENFORCEMENT, from_legacy_type,
};

const PATTERN_KEYS: [&str; 16] = [
    "id",
    "title",
    "intent",
    "scope",
    "requirement",
    "enforcement",
    "severity",
    "evidence",
    "exceptions",
    "options",
    "tuning",
    "implementation",
    "fix",
    "citation",
    "strict",
    "examples",
];
const OPTION_KEYS: [&str; 4] = ["type", "default", "description", "per_language"];
const OVERRIDE_OPTION_KEYS: [&str; 2] = ["default", "per_language"];
const PACK_KEYS: [&str; 4] = ["id", "title", "intro", "sections"];
const SECTION_KEYS: [&str; 4] = ["id", "title", "intro", "patterns"];
const SOURCE_KEYS: [&str; 5] = ["ref", "text", "patterns", "omitted", "note"];
const IMPLEMENTATION_KEYS: [&str; 2] = ["builtin", "declarative"];
const RULE_KEYS: [&str; 4] = ["select", "where", "message", "evidence"];
const FIX_KEYS: [&str; 5] = ["safety", "requires", "ops", "command", "rpc"];
const COMMAND_KEYS: [&str; 6] = ["argv", "output", "stdin", "env", "timeout", "scope"];
/// The keys an override file may carry; `extends` marks it.
const OVERRIDE_KEYS: [&str; 6] = [
    "severity",
    "exceptions",
    "options",
    "tuning",
    "examples",
    "extends",
];

type Converted = Result<Value, String>;

/// Whether a legacy document is an override: `extends` and at least one other
/// key an override has.
pub fn is_override(doc: &Value) -> bool {
    let Some(map) = doc.as_mapping() else {
        return false;
    };
    map.get("extends").is_some_and(Value::is_string)
        && map.keys().any(|key| {
            key.as_str()
                .is_some_and(|k| k != "extends" && OVERRIDE_KEYS.contains(&k))
        })
}

/// Whether a document already is a resource.
pub fn is_resource(doc: &Value) -> bool {
    doc.get("apiVersion").is_some()
}

/// A `Decision` document from a pattern file of `section`. A pattern whose
/// implementation is `declarative` needs the mapping of its rule file as
/// `rule`, with the name the file went by.
pub fn migrate_decision(old: &Value, section: &str, rule: Option<(&str, &Value)>) -> Converted {
    let old = mapping(old, "a pattern")?;
    only(old, &PATTERN_KEYS, "a pattern")?;
    let metadata = decision_metadata(text(old, "id")?, section, rule.map(|(file, _)| file));
    let mut decision = document("Decision", metadata, decision_spec(old, rule)?);
    convert_enforcement(&mut decision)?;
    Ok(decision)
}

/// Whether a `Decision` document still has the `enforcement` that `severity`
/// replaced.
pub fn has_enforcement(doc: &Value) -> bool {
    doc.get("kind").and_then(Value::as_str) == Some("Decision")
        && doc
            .get("spec")
            .is_some_and(|spec| spec.get("enforcement").is_some())
}

/// Rewrites `spec.enforcement` of a `Decision` document as `severity`: the
/// old default (mechanical `error`, heuristic `warn`, judgment `info`) unless
/// the decision overrode it; `doc` becomes a decision without a check. A
/// decision that had no check becomes a `model` check (review is how it was
/// enforced), capped at `warn`. Where the old enforcement cannot be
/// read back from the new severity it is kept in an annotation, so verdicts
/// recorded before keep applying. A document without `enforcement` is left as
/// it is.
pub fn convert_enforcement(doc: &mut Value) -> Result<(), String> {
    let Some(old) = doc.get("spec").and_then(Value::as_mapping).cloned() else {
        return Ok(());
    };
    let Some(enforcement) = old.get("enforcement").and_then(Value::as_str) else {
        return Ok(());
    };
    let has_check = old.get("check").is_some();
    let overridden = old.get("severity").and_then(Value::as_str);
    let severity = authored_severity(enforcement, overridden, has_check)?;
    let judged = severity.is_some() && !has_check;
    let spec = rebuilt(&old, severity.as_deref(), judged);
    let Some(root) = doc.as_mapping_mut() else {
        return Ok(());
    };
    root.insert("spec".into(), Value::Mapping(spec));
    if enforcement_of(severity.as_deref()) != enforcement {
        remember_enforcement(root, enforcement);
    }
    Ok(())
}

/// A `DecisionOverride` document named `name` from a pattern file with
/// `extends`.
pub fn migrate_override(old: &Value, name: &str) -> Converted {
    let old = mapping(old, "an override")?;
    only(old, &OVERRIDE_KEYS, "an override")?;
    let mut metadata = Mapping::new();
    put(&mut metadata, "name", name);
    let mut spec = Mapping::new();
    copy(&mut spec, old, "extends");
    if let Some(severity) = old.get("severity") {
        put(&mut spec, "severity", level(severity));
    }
    copy(&mut spec, old, "exceptions");
    let mut options = Mapping::new();
    let mut languages: BTreeMap<String, (Mapping, Option<Value>)> = BTreeMap::new();
    if let Some(Value::Mapping(old_options)) = old.get("options") {
        for (name, change) in old_options {
            let Value::Mapping(change) = change else {
                return Err("an override option is a mapping".to_owned());
            };
            only(change, &OVERRIDE_OPTION_KEYS, "an override option")?;
            if let Some(default) = change.get("default") {
                options.insert(name.clone(), default.clone());
            }
            if let Some(Value::Mapping(per)) = change.get("per_language") {
                for (language, value) in per {
                    let entry = languages.entry(key_text(language)).or_default();
                    entry.0.insert(name.clone(), value.clone());
                }
            }
        }
    }
    if let Some(Value::Mapping(tuning)) = old.get("tuning") {
        for (language, wording) in tuning {
            languages.entry(key_text(language)).or_default().1 = Some(wording.clone());
        }
    }
    if !options.is_empty() {
        put(&mut spec, "options", Value::Mapping(options));
    }
    let languages = language_mapping(languages);
    if !languages.is_empty() {
        put(&mut spec, "languages", Value::Mapping(languages));
    }
    copy(&mut spec, old, "examples");
    Ok(document("DecisionOverride", metadata, spec))
}

/// A `Pack` document from `pack.yaml` and the `section.yaml` files it lists,
/// by section id. `overrides` names the files a section listed that are
/// overrides: they are documents of their own now and no longer listed.
pub fn migrate_pack(
    pack: &Value,
    sections: &BTreeMap<String, Value>,
    overrides: &std::collections::BTreeSet<String>,
) -> Converted {
    let pack = mapping(pack, "a pack")?;
    only(pack, &PACK_KEYS, "a pack")?;
    let mut metadata = Mapping::new();
    put(&mut metadata, "name", text(pack, "id")?);
    let mut spec = Mapping::new();
    copy(&mut spec, pack, "title");
    copy(&mut spec, pack, "intro");
    let Some(Value::Sequence(listed)) = pack.get("sections") else {
        return Err("a pack lists its `sections`".to_owned());
    };
    let mut out = Vec::new();
    for name in listed {
        let name = name.as_str().ok_or("a section name is a string")?;
        let section = sections
            .get(name)
            .ok_or_else(|| format!("section `{name}` has no section.yaml"))?;
        let section = mapping(section, "a section")?;
        only(section, &SECTION_KEYS, "a section")?;
        let mut entry = Mapping::new();
        put(&mut entry, "name", name);
        copy(&mut entry, section, "title");
        copy(&mut entry, section, "intro");
        if let Some(Value::Sequence(patterns)) = section.get("patterns") {
            let kept: Vec<Value> = patterns
                .iter()
                .filter(|p| p.as_str().is_none_or(|name| !overrides.contains(name)))
                .cloned()
                .collect();
            put(&mut entry, "decisions", Value::Sequence(kept));
        }
        out.push(Value::Mapping(entry));
    }
    put(&mut spec, "sections", Value::Sequence(out));
    Ok(document("Pack", metadata, spec))
}

/// A `SourceMap` document from `sources.yaml`.
pub fn migrate_sources(old: &Value) -> Converted {
    let Value::Sequence(list) = old else {
        return Err("sources.yaml is a list".to_owned());
    };
    let mut sources = Vec::new();
    for item in list {
        let item = mapping(item, "a source")?;
        only(item, &SOURCE_KEYS, "a source")?;
        let mut source = Mapping::new();
        for key in ["ref", "text"] {
            copy(&mut source, item, key);
        }
        if let Some(patterns) = item.get("patterns") {
            put(&mut source, "decisions", patterns.clone());
        }
        copy(&mut source, item, "omitted");
        copy(&mut source, item, "note");
        sources.push(Value::Mapping(source));
    }
    let mut metadata = Mapping::new();
    put(&mut metadata, "name", "sources");
    let mut spec = Mapping::new();
    put(&mut spec, "sources", Value::Sequence(sources));
    Ok(document("SourceMap", metadata, spec))
}

fn decision_metadata(id: &str, section: &str, rule_file: Option<&str>) -> Mapping {
    let pack = id.split_once('/').map_or(id, |(pack, _)| pack);
    let mut labels = Mapping::new();
    put(&mut labels, PACK_LABEL, pack);
    put(&mut labels, SECTION_LABEL, section);
    let mut metadata = Mapping::new();
    put(&mut metadata, "name", id);
    put(&mut metadata, "labels", Value::Mapping(labels));
    if let Some(file) = rule_file {
        let mut annotations = Mapping::new();
        put(&mut annotations, MIGRATED_FROM, file);
        put(&mut metadata, "annotations", Value::Mapping(annotations));
    }
    metadata
}

fn decision_spec(old: &Mapping, rule: Option<(&str, &Value)>) -> Result<Mapping, String> {
    let mut spec = Mapping::new();
    for key in ["title", "intent"] {
        copy(&mut spec, old, key);
    }
    let mut scope = Mapping::new();
    put(&mut scope, "domain", "code");
    put(&mut scope, "subject", text(old, "scope")?);
    put(&mut spec, "scope", Value::Mapping(scope));
    for key in ["requirement", "enforcement"] {
        copy(&mut spec, old, key);
    }
    if let Some(severity) = old.get("severity") {
        put(&mut spec, "severity", level(severity));
    }
    copy(&mut spec, old, "evidence");
    copy(&mut spec, old, "exceptions");
    let languages = languages(old)?;
    if let Some(options) = old.get("options") {
        put(&mut spec, "options", schema(options)?);
    }
    if !languages.is_empty() {
        put(&mut spec, "languages", Value::Mapping(languages));
    }
    if let Some(implementation) = old.get("implementation") {
        put(&mut spec, "check", check(implementation, rule)?);
    }
    if let Some(fix) = old.get("fix") {
        put(&mut spec, "fix", migrate_fix(fix)?);
    }
    for key in ["citation", "strict", "examples"] {
        copy(&mut spec, old, key);
    }
    Ok(spec)
}

/// The severity a decision of `enforcement` authors: its default unless it
/// overrode it, and `warn` at most without a check.
fn authored_severity(
    enforcement: &str,
    overridden: Option<&str>,
    has_check: bool,
) -> Result<Option<String>, String> {
    let default = match enforcement {
        "mechanical" => "error",
        "heuristic" => "warn",
        "judgment" => "info",
        "doc" if has_check => return Err("a `doc` decision has no check".to_owned()),
        "doc" => return Ok(None),
        other => return Err(format!("unknown enforcement `{other}`")),
    };
    let severity = overridden.unwrap_or(default);
    let capped = if !has_check && severity == "error" {
        "warn"
    } else {
        severity
    };
    Ok(Some(capped.to_owned()))
}

/// The enforcement a severity reads back as.
fn enforcement_of(severity: Option<&str>) -> &'static str {
    match severity {
        None => "doc",
        Some("error") => "mechanical",
        Some("warn") => "heuristic",
        _ => "judgment",
    }
}

/// `spec` with `enforcement` replaced by `severity` and, for a decision that
/// review enforces, a `model` check placed before the keys that follow it.
fn rebuilt(old: &Mapping, severity: Option<&str>, judged: bool) -> Mapping {
    let mut spec = Mapping::new();
    let mut placed = false;
    for (key, value) in old {
        match key.as_str() {
            Some("enforcement") => {
                if let Some(severity) = severity {
                    put(&mut spec, "severity", severity);
                }
            }
            Some("severity") => {}
            Some("fix" | "citation" | "strict" | "examples") if judged && !placed => {
                put(&mut spec, "check", judge_check());
                placed = true;
                spec.insert(key.clone(), value.clone());
            }
            _ => {
                spec.insert(key.clone(), value.clone());
            }
        }
    }
    if judged && !placed {
        put(&mut spec, "check", judge_check());
    }
    spec
}

/// Keeps the old `enforcement` in an annotation of the document.
fn remember_enforcement(root: &mut Mapping, enforcement: &str) {
    let Some(Value::Mapping(metadata)) = root.get_mut("metadata") else {
        return;
    };
    let annotations = metadata
        .entry("annotations".into())
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    if let Value::Mapping(annotations) = annotations {
        put(annotations, WAS_ENFORCEMENT, enforcement);
    }
}

/// The check of a decision that review enforced: a model asked its
/// requirement over the subjects of its scope.
fn judge_check() -> Value {
    let mut check = Mapping::new();
    put(&mut check, "type", "model");
    Value::Mapping(check)
}

fn document(kind: &str, metadata: Mapping, spec: Mapping) -> Value {
    let mut doc = Mapping::new();
    put(&mut doc, "apiVersion", API_VERSION);
    put(&mut doc, "kind", kind);
    put(&mut doc, "metadata", Value::Mapping(metadata));
    put(&mut doc, "spec", Value::Mapping(spec));
    Value::Mapping(doc)
}

/// `type`, `default` and `description` per option as a JSON Schema object.
fn schema(options: &Value) -> Converted {
    let options = mapping(options, "`options`")?;
    let mut properties = Mapping::new();
    for (name, spec) in options {
        let spec = mapping(spec, "an option")?;
        let mut property = Mapping::new();
        only(spec, &OPTION_KEYS, "an option")?;
        let legacy = text(spec, "type")?;
        let kind =
            from_legacy_type(legacy).ok_or_else(|| format!("unknown option type `{legacy}`"))?;
        put(&mut property, "type", kind.name());
        copy(&mut property, spec, "default");
        copy(&mut property, spec, "description");
        properties.insert(name.clone(), Value::Mapping(property));
    }
    let mut schema = Mapping::new();
    put(&mut schema, "type", "object");
    put(&mut schema, "properties", Value::Mapping(properties));
    put(&mut schema, "additionalProperties", false);
    Ok(Value::Mapping(schema))
}

/// The per-language option values and wording of a pattern.
fn languages(old: &Mapping) -> Result<Mapping, String> {
    let mut by_language: BTreeMap<String, (Mapping, Option<Value>)> = BTreeMap::new();
    if let Some(Value::Mapping(options)) = old.get("options") {
        for (name, spec) in options {
            let Value::Mapping(spec) = spec else {
                return Err("an option is a mapping".to_owned());
            };
            if let Some(Value::Mapping(per)) = spec.get("per_language") {
                for (language, value) in per {
                    by_language
                        .entry(key_text(language))
                        .or_default()
                        .0
                        .insert(name.clone(), value.clone());
                }
            }
        }
    }
    if let Some(Value::Mapping(tuning)) = old.get("tuning") {
        for (language, wording) in tuning {
            by_language.entry(key_text(language)).or_default().1 = Some(wording.clone());
        }
    }
    Ok(language_mapping(by_language))
}

fn language_mapping(by_language: BTreeMap<String, (Mapping, Option<Value>)>) -> Mapping {
    let mut out = Mapping::new();
    for (language, (options, tuning)) in by_language {
        let mut entry = Mapping::new();
        if !options.is_empty() {
            put(&mut entry, "options", Value::Mapping(options));
        }
        if let Some(tuning) = tuning {
            put(&mut entry, "tuning", tuning);
        }
        put(&mut out, &language, Value::Mapping(entry));
    }
    out
}

fn check(implementation: &Value, rule: Option<(&str, &Value)>) -> Converted {
    let implementation = mapping(implementation, "`implementation`")?;
    only(implementation, &IMPLEMENTATION_KEYS, "`implementation`")?;
    let mut check = Mapping::new();
    match (
        implementation.get("builtin"),
        implementation.get("declarative"),
    ) {
        (Some(id), None) => {
            put(&mut check, "type", "builtin");
            put(&mut check, "id", id.clone());
        }
        (None, Some(_)) => {
            let (_, rule) = rule.ok_or("the declarative rule file is missing")?;
            let rule = mapping(rule, "a rule file")?;
            only(rule, &RULE_KEYS, "a rule file")?;
            put(&mut check, "type", "cel");
            copy(&mut check, rule, "select");
            copy(&mut check, rule, "where");
            copy(&mut check, rule, "message");
            copy(&mut check, rule, "evidence");
        }
        _ => return Err("an implementation has one of `builtin` or `declarative`".to_owned()),
    }
    Ok(Value::Mapping(check))
}

fn migrate_fix(fix: &Value) -> Converted {
    let fix = mapping(fix, "`fix`")?;
    only(fix, &FIX_KEYS, "`fix`")?;
    let mut out = Mapping::new();
    copy(&mut out, fix, "safety");
    copy(&mut out, fix, "requires");
    match (fix.get("ops"), fix.get("command"), fix.get("rpc")) {
        (Some(ops), None, None) => {
            put(&mut out, "type", "ops");
            put(&mut out, "ops", ops.clone());
        }
        (None, Some(Value::Mapping(command)), None) => {
            only(command, &COMMAND_KEYS, "a fix command")?;
            put(&mut out, "type", "command");
            copy(&mut out, command, "argv");
            if let Some(mode) = command.get("output") {
                put(&mut out, "output", kebab_case(mode));
            }
            copy(&mut out, command, "stdin");
            copy(&mut out, command, "env");
            if let Some(timeout) = command.get("timeout") {
                put(&mut out, "timeout", duration(timeout)?);
            }
            copy(&mut out, command, "scope");
        }
        (None, None, Some(rpc)) => {
            put(&mut out, "type", "rpc");
            if rpc.is_mapping() {
                put(&mut out, "params", rpc.clone());
            }
        }
        _ => return Err("a fix has one of `ops`, `command` or `rpc`".to_owned()),
    }
    Ok(Value::Mapping(out))
}

/// `review` was a severity; it is `info` now.
fn level(value: &Value) -> Value {
    match value.as_str() {
        Some("review") => Value::String("info".to_owned()),
        _ => value.clone(),
    }
}

/// `inPlace` becomes `in-place`.
fn kebab_case(value: &Value) -> Value {
    match value.as_str() {
        Some("inPlace") => Value::String("in-place".to_owned()),
        _ => value.clone(),
    }
}

/// A bare whole number of seconds becomes `30s`; anything else that is a
/// number is refused rather than rounded.
fn duration(value: &Value) -> Converted {
    match value {
        Value::Number(n) if n.is_u64() => Ok(Value::String(format!("{n}s"))),
        Value::Number(n) => Err(format!("a timeout is whole seconds, found `{n}`")),
        Value::String(text) if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) => {
            Ok(Value::String(format!("{text}s")))
        }
        other => Ok(other.clone()),
    }
}

/// Refuses a key of `map` that is not in `allowed`, so a migration never
/// drops what it does not understand.
fn only(map: &Mapping, allowed: &[&str], what: &str) -> Result<(), String> {
    for key in map.keys() {
        let name = key.as_str().unwrap_or_default();
        if !allowed.contains(&name) {
            return Err(format!("unknown key `{name}` in {what}"));
        }
    }
    Ok(())
}

fn mapping<'a>(value: &'a Value, what: &str) -> Result<&'a Mapping, String> {
    value
        .as_mapping()
        .ok_or_else(|| format!("{what} is a mapping"))
}

fn text<'a>(map: &'a Mapping, key: &str) -> Result<&'a str, String> {
    map.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{key}` is missing or not text"))
}

fn key_text(key: &Value) -> String {
    key.as_str().unwrap_or_default().to_owned()
}

fn put(map: &mut Mapping, key: &str, value: impl Into<Value>) {
    map.insert(Value::String(key.to_owned()), value.into());
}

fn copy(into: &mut Mapping, from: &Mapping, key: &str) {
    if let Some(value) = from.get(key) {
        put(into, key, value.clone());
    }
}
