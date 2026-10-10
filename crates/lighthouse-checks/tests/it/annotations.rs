//! Source annotations, finding identity and finding facts, through the engine
//! and the Rust provider.

use std::fs;

use lighthouse_config::Config;
use lighthouse_engine::{Engine, Outcome};
use lighthouse_model::Severity;

/// Checks a one-file crate with the design and core rules plus `rules`.
fn check(source: &str, rules: &str) -> Outcome {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), source).unwrap();
    let plugin = lighthouse_test_support::lang_rust();
    let config = Config::parse_inline(&format!(
        "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"design\", \"core\"]\n\
         extends = [\"core/recommended\"]\n[rules]\n{rules}",
        plugin.to_str().unwrap()
    ))
    .unwrap();
    let mut registry = lighthouse_checks::registry();
    lighthouse_rpc::register(&mut registry, &config, dir.path(), &[]).unwrap();
    let outcome = Engine::new(registry, config, dir.path())
        .unwrap()
        .check(&[], &[])
        .unwrap();
    assert!(outcome.incomplete.is_empty(), "{:?}", outcome.incomplete);
    outcome
}

const DOC_RULE: &str = "\"design/exported-doc\" = \"warn\"\n";

fn rules_of(outcome: &Outcome) -> Vec<&str> {
    outcome
        .diagnostics
        .iter()
        .map(|d| d.rule_id.as_str())
        .collect()
}

#[test]
fn an_allow_annotation_suppresses_the_finding_it_is_attached_to() {
    let outcome = check(
        "// lighthouse:allow design/exported-doc -- documented at its origin\npub fn open() {}\n",
        DOC_RULE,
    );
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    assert_eq!(outcome.allowed.len(), 1);
    let allowed = &outcome.allowed[0];
    assert_eq!(allowed.diagnostic.rule_id, "design/exported-doc");
    assert_eq!(allowed.reason, "documented at its origin");

    let elsewhere = check(
        "// lighthouse:allow design/exported-doc -- only the next one\npub fn open() {}\n\npub fn close() {}\n",
        DOC_RULE,
    );
    assert_eq!(rules_of(&elsewhere), ["design/exported-doc"]);
    assert_eq!(elsewhere.diagnostics[0].span.start.line, 4);
    assert_eq!(elsewhere.allowed.len(), 1);
}

#[test]
fn an_allow_annotation_covers_every_severity_and_names_the_rules_it_does_not_use() {
    let outcome = check(
        "// lighthouse:allow design/exported-doc, design/no-exported-mutable-global -- generated\npub fn open() {}\n",
        "\"design/exported-doc\" = \"error\"\n\"design/no-exported-mutable-global\" = \"warn\"\n",
    );
    assert_eq!(rules_of(&outcome), ["core/unused-allow"]);
    assert!(
        outcome.diagnostics[0]
            .message
            .contains("no-exported-mutable-global")
    );
    assert_eq!(outcome.allowed.len(), 1);
    assert_eq!(outcome.allowed[0].diagnostic.severity, Severity::Error);
    assert_eq!(outcome.exit_code(false, false), 0);
}

#[test]
fn an_annotation_without_a_reason_is_ignored_and_reported() {
    let outcome = check(
        "// lighthouse:allow design/exported-doc\npub fn open() {}\n",
        DOC_RULE,
    );
    assert_eq!(
        rules_of(&outcome),
        ["core/annotation-reason", "design/exported-doc"]
    );
    assert!(outcome.allowed.is_empty());
    assert_eq!(outcome.diagnostics[0].severity, Severity::Error);
    assert_eq!(outcome.diagnostics[0].span.start.line, 1);
}

#[test]
fn an_annotation_that_suppresses_nothing_is_reported_as_unused() {
    let outcome = check(
        "// lighthouse:allow design/exported-doc -- stale\n/// Documented.\npub fn open() {}\n",
        DOC_RULE,
    );
    assert_eq!(rules_of(&outcome), ["core/unused-allow"]);
    assert_eq!(outcome.diagnostics[0].severity, Severity::Warn);
    assert!(
        outcome.diagnostics[0]
            .message
            .contains("design/exported-doc")
    );

    let disabled = check(
        "// lighthouse:allow design/exported-doc -- the rule is off\npub fn open() {}\n",
        "",
    );
    assert_eq!(rules_of(&disabled), ["core/unused-allow"]);
}

#[test]
fn prose_mentioning_the_marker_is_not_an_annotation() {
    let outcome = check(
        "// Write lighthouse:allow design/exported-doc -- why, to allow a finding.\npub fn open() {}\n",
        DOC_RULE,
    );
    assert_eq!(rules_of(&outcome), ["design/exported-doc"]);
    assert!(outcome.allowed.is_empty());
}

fn banner_in(name: &str) -> String {
    format!("fn {name}() {{\n    // ======== helpers ========\n    let _ = 1;\n}}\n\n")
}

#[test]
fn identical_findings_in_different_symbols_keep_their_fingerprints_when_one_goes() {
    let rules = "\"design/section-banners\" = \"warn\"\n";
    let all = check(
        &format!("{}{}{}", banner_in("a"), banner_in("b"), banner_in("c")),
        rules,
    );
    assert_eq!(all.diagnostics.len(), 3, "{:?}", all.diagnostics);
    let fingerprints: Vec<_> = all.diagnostics.iter().map(|d| &d.fingerprint).collect();
    assert_ne!(fingerprints[0], fingerprints[1]);
    assert_ne!(fingerprints[1], fingerprints[2]);
    assert!(all.facts[fingerprints[1]].get("ordinal").is_none());

    let without_a = check(&format!("{}{}", banner_in("b"), banner_in("c")), rules);
    let remaining: Vec<_> = without_a
        .diagnostics
        .iter()
        .map(|d| &d.fingerprint)
        .collect();
    assert_eq!(remaining, [fingerprints[1], fingerprints[2]]);
}

#[test]
fn identical_findings_in_one_symbol_rest_on_an_ordinal_and_say_so() {
    let outcome = check(
        "fn a() {\n    // ======== helpers ========\n    let _ = 1;\n    // ======== helpers ========\n    let _ = 2;\n}\n",
        "\"design/section-banners\" = \"warn\"\n",
    );
    assert_eq!(outcome.diagnostics.len(), 2);
    for d in &outcome.diagnostics {
        assert_eq!(outcome.facts[&d.fingerprint]["ordinal"], true);
    }
    assert_ne!(
        outcome.diagnostics[0].fingerprint,
        outcome.diagnostics[1].fingerprint
    );
}

#[test]
fn facts_describe_the_subject_with_callers_split_and_file_measures() {
    let outcome = check("pub fn open() {}\n", DOC_RULE);
    let facts = &outcome.facts[&outcome.diagnostics[0].fingerprint];
    assert_eq!(facts["language"], "rust");
    assert_eq!(facts["kind"], "function");
    assert_eq!(facts["callers_same_module"], 0);
    assert_eq!(facts["callers_other_module"], 0);

    let long = "// a\n".repeat(5);
    let lines = check(
        &format!("{long}pub fn open() {{}}\n"),
        "\"core/max-file-lines\" = { level = \"warn\", options = { max = 2 } }\n",
    );
    let finding = lines
        .diagnostics
        .iter()
        .find(|d| d.rule_id == "core/max-file-lines" && d.file.ends_with("lib.rs"))
        .unwrap();
    let file = &lines.facts[&finding.fingerprint]["file"];
    assert_eq!(file["lines"], 6);
    assert_eq!(file["path"], "src/lib.rs");
}

#[test]
fn outcome_lists_configured_rules_and_the_options_each_finding_ran_with() {
    let outcome = check(
        "pub fn open() {}\n",
        "\"design/exported-doc\" = \"warn\"\n\"core/max-file-lines\" = { level = \"warn\", options = { max = 1 } }\n",
    );
    assert!(
        outcome
            .configured
            .contains(&"design/exported-doc".to_owned())
    );
    assert!(
        outcome
            .configured
            .contains(&"core/max-file-lines".to_owned())
    );
    let doc = outcome
        .diagnostics
        .iter()
        .find(|d| d.rule_id == "design/exported-doc")
        .unwrap();
    assert!(outcome.options[&doc.fingerprint].is_empty());
}
