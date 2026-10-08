mod support;

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_resource::{header, to_yaml, yaml_values};
use lighthouse_spec::{
    Catalog, Check, FixKind, migrate_decision, migrate_override, migrate_pack, migrate_sources,
};
use serde_json::json;
use serde_norway::Value;
use sha2::{Digest, Sha256};
use support::Files;

const OLD: &str = "id: p/a
title: A
intent: Why.
scope: file
requirement: >-
  A MUST
  b.
enforcement: heuristic
severity: review
evidence: [x]
options:
  max:
    type: int
    default: 3
    description: Limit.
    per_language: { go: 4 }
  kinds:
    type: list
    default: [a]
    description: Kinds.
tuning:
  go: Go wording.
implementation:
  builtin: p/a
fix:
  safety: suggested
  command:
    argv: [gofmt, -w, '{file}']
    output: inPlace
    timeout: 30
examples:
  - name: bad
    language: go
    kind: invalid
    files: [{ path: a.go, body: x }]
    expect: [{ line: 1 }]
    fixed: [{ path: a.go, body: y }]
  - name: good
    language: go
    kind: valid
    files: [{ path: a.go, body: y }]
";

fn value(text: &str) -> Value {
    yaml_values("test", text).unwrap().remove(0)
}

fn catalog_of(decision: &Value) -> Catalog {
    let mut files = Files::new();
    let pack = support::pack("p", &[("s", &["a"])]);
    files.insert("p/pack.yaml".to_owned(), pack);
    files.insert("p/s/a.yaml".to_owned(), to_yaml(decision));
    Catalog::from_files(files).unwrap()
}

/// What a build from before the resource model hashed for the pattern in
/// `OLD`: its requirement, enforcement, options and implementation.
fn the_old_semantic_version() -> String {
    let content = json!({
        "requirement": "A MUST b.",
        "enforcement": "heuristic",
        "options": {
            "max": { "type": "int", "default": 3, "description": "Limit.", "per_language": { "go": 4 } },
            "kinds": { "type": "list", "default": ["a"], "description": "Kinds." },
        },
        "implementation": { "builtin": "p/a" },
    });
    Sha256::digest(content.to_string().as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn a_pattern_becomes_a_decision_that_loads() {
    let migrated = migrate_decision(&value(OLD), "s", None).unwrap();

    let catalog = catalog_of(&migrated);

    let decision = catalog.decision("p/a").unwrap();
    assert_eq!(decision.pack(), "p");
    assert_eq!(decision.section(), "s");
    assert_eq!(decision.scope.subject.to_string(), "file");
    assert_eq!(
        decision.severity,
        Some(lighthouse_model::Severity::Info),
        "`review` is `info`"
    );
    let options = decision.options.as_ref().unwrap();
    assert_eq!(options.properties["max"].kind.name(), "integer");
    assert_eq!(options.properties["kinds"].kind.name(), "array");
    assert_eq!(decision.languages["go"].options["max"], 4);
    assert_eq!(
        decision.languages["go"].tuning.as_deref(),
        Some("Go wording.")
    );
    assert!(matches!(&decision.check, Some(Check::Builtin(b)) if b.id == "p/a"));
    let FixKind::Command(command) = &decision.fix.as_ref().unwrap().kind else {
        panic!("command expected");
    };
    assert_eq!(
        command.timeout, "30s",
        "a bare number of seconds is a duration"
    );
    assert_eq!(serde_json::to_value(command.output).unwrap(), "in-place");
}

#[test]
fn a_migrated_decision_keeps_the_semantic_version_of_before() {
    let migrated = migrate_decision(&value(OLD), "s", None).unwrap();

    let decision = catalog_of(&migrated).decision("p/a").unwrap().clone();

    assert_eq!(
        decision.legacy_semantic_version().unwrap(),
        the_old_semantic_version()
    );
    assert_ne!(
        decision.semantic_version(),
        the_old_semantic_version(),
        "the hash of the new content is a different number: that is why the old one is kept"
    );
}

#[test]
fn migrated_text_reads_back_as_the_value_it_was_written_from() {
    let migrated = migrate_decision(&value(OLD), "s", None).unwrap();
    let text = format!("{}{}", header("Decision", "../schema"), to_yaml(&migrated));
    assert!(text.starts_with("# yaml-language-server: $schema=../schema/decision.schema.json\n"));
    assert_eq!(value(&text), migrated);
}

#[test]
fn a_declarative_rule_file_is_inlined_and_remembered() {
    let old = OLD.replace("builtin: p/a", "declarative: p/s/rules/a.yaml");
    let rule = value(
        "select: file\nwhere: 'true'\nmessage: m {{ file.path }}\nevidence: { path: file.path }\n",
    );

    let migrated = migrate_decision(&value(&old), "s", Some(("p/s/rules/a.yaml", &rule))).unwrap();

    let decision = catalog_of(&migrated).decision("p/a").unwrap().clone();
    let Some(Check::Cel(cel)) = &decision.check else {
        panic!("cel expected");
    };
    assert_eq!(cel.select.name(), "file");
    assert_eq!(cel.evidence["path"], "file.path");
    assert!(decision.legacy_semantic_version().is_some());
    assert_eq!(
        decision.metadata().annotations[lighthouse_spec::MIGRATED_FROM],
        "p/s/rules/a.yaml"
    );
}

#[test]
fn a_pack_and_its_sections_become_one_pack_that_lists_the_decisions() {
    let pack = value("id: p\ntitle: P\nintro: x\nsections: [s, t]\n");
    let sections: BTreeMap<String, Value> = [
        (
            "s".to_owned(),
            value("id: s\ntitle: S\nintro: i\npatterns: [a, tweak]\n"),
        ),
        (
            "t".to_owned(),
            value("id: t\ntitle: T\nintro: j\npatterns: []\n"),
        ),
    ]
    .into();
    let overrides = BTreeSet::from(["tweak".to_owned()]);

    let migrated = migrate_pack(&pack, &sections, &overrides).unwrap();

    let migrated = serde_json::to_value(&migrated).unwrap();
    assert_eq!(migrated["kind"], "Pack");
    assert_eq!(migrated["spec"]["sections"][0]["decisions"], json!(["a"]));
    assert_eq!(migrated["spec"]["sections"][1]["name"], "t");
    let missing = migrate_pack(&pack, &BTreeMap::new(), &overrides).unwrap_err();
    assert!(missing.contains("no section.yaml"), "{missing}");
}

#[test]
fn an_override_keeps_what_it_changed() {
    let old = value(
        "extends: p/a\nseverity: review\nexceptions: Vendored.\ntuning: { go: Local. }\noptions:\n  max:\n    default: 9\n    per_language: { go: 5 }\n",
    );

    let migrated = migrate_override(&old, "tweak").unwrap();

    let migrated = serde_json::to_value(&migrated).unwrap();
    assert_eq!(migrated["kind"], "DecisionOverride");
    assert_eq!(migrated["metadata"]["name"], "tweak");
    assert_eq!(migrated["spec"]["severity"], "info");
    assert_eq!(migrated["spec"]["options"], json!({ "max": 9 }));
    assert_eq!(
        migrated["spec"]["languages"],
        json!({ "go": { "options": { "max": 5 }, "tuning": "Local." } })
    );
}

#[test]
fn sources_become_a_source_map_of_decisions() {
    let old = value(
        "- ref: d#a-1\n  text: t\n  patterns: [p/a]\n- ref: d#b-2\n  text: u\n  omitted: why\n",
    );

    let migrated = migrate_sources(&old).unwrap();

    let migrated = serde_json::to_value(&migrated).unwrap();
    assert_eq!(migrated["kind"], "SourceMap");
    assert_eq!(migrated["spec"]["sources"][0]["decisions"], json!(["p/a"]));
    assert_eq!(migrated["spec"]["sources"][1]["omitted"], "why");
}

#[test]
fn a_fractional_timeout_is_refused_instead_of_rounded() {
    let old = OLD.replace("timeout: 30", "timeout: 1.5");
    let error = migrate_decision(&value(&old), "s", None).unwrap_err();
    assert!(error.contains("whole seconds"), "{error}");
}

#[test]
fn a_key_the_migration_does_not_know_is_an_error() {
    let old = format!("{OLD}bogus: 1\n");
    let error = migrate_decision(&value(&old), "s", None).unwrap_err();
    assert!(error.contains("unknown key `bogus`"), "{error}");
    let error = migrate_override(&value("extends: p/a\nbogus: 1\n"), "a").unwrap_err();
    assert!(error.contains("unknown key `bogus`"), "{error}");
}

#[test]
fn is_override_needs_extends_and_a_key_of_an_override() {
    assert!(lighthouse_spec::is_override(&value(
        "extends: p/a\nseverity: warn\n"
    )));
    assert!(!lighthouse_spec::is_override(&value("extends: p/a\n")));
    assert!(!lighthouse_spec::is_override(&value(
        "extends: [x]\nrules: {}\n"
    )));
}
