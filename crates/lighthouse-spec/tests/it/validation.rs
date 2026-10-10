//! The rules a decision must satisfy to enter the catalog.

use lighthouse_spec::{Catalog, Decision};
use lighthouse_test_support::catalog::*;
use serde_json::{Map, json};

#[test]
fn a_decision_about_decisions_is_in_the_spec_domain() {
    let in_domain = |domain: &str| {
        decision(
            "p/a",
            "s",
            &SPEC.replace(
                "scope: { subject: file }",
                &format!("scope: {{ domain: {domain}, subject: decision }}"),
            ),
        )
    };
    rejected(
        with("p/s/a.yaml", &in_domain("code")),
        "`decision` decision is in the `spec` domain, not `code`",
    );
    assert!(Catalog::from_files(with("p/s/a.yaml", &in_domain("spec"))).is_ok());
}

#[test]
fn a_uid_is_a_uuid_v4_and_belongs_to_one_decision() {
    let with_uid = |id: &str, uid: &str| {
        decision(id, "s", SPEC).replace(
            &format!("  name: {id}\n"),
            &format!("  name: {id}\n  uid: {uid}\n"),
        )
    };
    let uid = "5d6b1c1e-2b0e-4a43-9a3e-0f1b6f5d2a11";
    rejected(
        with("p/s/a.yaml", &with_uid("p/a", "not-a-uuid")),
        "not a lowercase UUID v4",
    );
    rejected(
        with(
            "p/s/a.yaml",
            &with_uid("p/a", "5D6B1C1E-2B0E-4A43-9A3E-0F1B6F5D2A11"),
        ),
        "not a lowercase UUID v4",
    );
    let mut twice = files(&[
        ("p/pack.yaml", pack("p", &[("s", &["a", "b"])])),
        ("p/s/a.yaml", with_uid("p/a", uid)),
        ("p/s/b.yaml", with_uid("p/b", uid)),
    ]);
    rejected(twice.clone(), "is also the uid of `p/a`");
    twice.insert(
        "p/s/b.yaml".to_owned(),
        with_uid("p/b", "0b0c5d8e-77d1-4f0f-8b61-6a4d2e9c3f10"),
    );
    assert!(Catalog::from_files(twice).is_ok());
}

const EXAMPLE_PAIR: &str = "  examples:
    - name: bad
      language: go
      kind: invalid
      files: [{ path: a.go, body: x }]
    - name: good
      language: go
      kind: valid
      files: [{ path: a.go, body: y }]
";

fn rejected(files: Files, needle: &str) {
    let err = Catalog::from_files(files).unwrap_err();
    assert!(
        err.to_string().contains(needle),
        "{err} should mention {needle}"
    );
}

fn swap(from: &str, to: &str) -> Files {
    with("p/s/a.yaml", &decision("p/a", "s", &SPEC.replace(from, to)))
}

#[test]
fn minimal_catalog_loads() {
    let catalog = Catalog::from_files(base()).unwrap();
    assert_eq!(catalog.decision("p/a").unwrap().severity(), None);
}

#[test]
fn a_decision_belongs_where_its_labels_and_its_pack_say() {
    let wrong_pack = decision("p/a", "s", SPEC).replace("lighthouse/pack: p", "lighthouse/pack: q");
    rejected(
        with("p/s/a.yaml", &wrong_pack),
        "label `lighthouse/pack` is `q`",
    );
    let wrong_section = decision("p/a", "t", SPEC);
    rejected(
        with("p/s/a.yaml", &wrong_section),
        "label `lighthouse/section` is `t`",
    );
    let unlabeled = decision("p/a", "s", SPEC).replace(
        "  labels:\n    lighthouse/pack: p\n    lighthouse/section: s\n",
        "",
    );
    rejected(with("p/s/a.yaml", &unlabeled), "needs the label");
}

#[test]
fn identity_comes_from_metadata_not_from_where_the_file_lies() {
    let mut files = base();
    let text = files.remove("p/s/a.yaml").unwrap();
    files.insert("anywhere/at/all/renamed.yaml".to_owned(), text);
    let catalog = Catalog::from_files(files).unwrap();
    assert_eq!(catalog.decision("p/a").unwrap().id(), "p/a");
}

#[test]
fn ids_must_be_unique() {
    let mut files = base();
    files.insert("p/s/copy.yaml".to_owned(), decision("p/a", "s", SPEC));
    rejected(files, "defined twice");
}

#[test]
fn decision_rules_are_enforced() {
    let severity_without_check = format!("{SPEC}{CHECKED_SPEC}");
    rejected(
        with("p/s/a.yaml", &decision("p/a", "s", &severity_without_check)),
        "no `severity`",
    );
    let check_without_severity = format!("{SPEC}  check:\n    type: builtin\n    id: p/a\n");
    rejected(
        with("p/s/a.yaml", &decision("p/a", "s", &check_without_severity)),
        "needs a `severity`",
    );
    rejected(swap("MUST", "must"), "MUST, SHOULD or MAY");
    rejected(swap("context: i", "context: ' '"), "context");
    let no_examples = "  severity: error\n  check:\n    type: builtin\n    id: p/a\n";
    rejected(
        with(
            "p/s/a.yaml",
            &decision("p/a", "s", &format!("{SPEC}{no_examples}")),
        ),
        "valid and an invalid example",
    );
    rejected(
        swap("title: A\n", "title: A\n  nonsense: 1\n"),
        "unknown field",
    );
}

#[test]
fn checked_decisions_need_a_valid_and_an_invalid_example() {
    let checked = "  check:\n    type: builtin\n    id: p/a\n";
    rejected(decision_with(checked), "valid and an invalid example");
    Catalog::from_files(decision_with(&format!("{checked}{EXAMPLE_PAIR}"))).unwrap();
    let judged = "  severity: info\n  check:\n    type: model\n";
    Catalog::from_files(with(
        "p/s/a.yaml",
        &decision("p/a", "s", &format!("{SPEC}{judged}")),
    ))
    .unwrap();
    rejected(
        decision_with("  check:\n    type: builtin\n    id: ''\n"),
        "needs a rule id",
    );
}

#[test]
fn a_cel_check_must_compile_and_select_what_the_scope_runs_over() {
    let cel = |select: &str, condition: &str| {
        format!(
            "  check:\n    type: cel\n    select: {select}\n    where: \"{condition}\"\n    message: m\n{EXAMPLE_PAIR}"
        )
    };
    Catalog::from_files(decision_with(&cel("file", "true"))).unwrap();
    rejected(decision_with(&cel("file", "1 +")), "check where");
    rejected(decision_with(&cel("module", "true")), "cannot run over");
    rejected(
        decision_with(&format!(
            "  check:\n    type: cel\n    select: file\n    where: 'true'\n    message: '{{{{ 1 + }}}}'\n{EXAMPLE_PAIR}"
        )),
        "check message",
    );
    rejected(
        decision_with("  check:\n    type: python\n    code: x\n"),
        "expected builtin, cel, command, rpc or model",
    );
}

#[test]
fn at_most_one_canonical_example_per_language_and_kind() {
    let example = |name: &str, language: &str, canonical: bool| {
        format!(
            "    - name: {name}\n      language: {language}\n      kind: valid\n      canonical: {canonical}\n      files: [{{ path: a, body: x }}]\n"
        )
    };
    let one = format!(
        "  examples:\n{}{}",
        example("a", "go", true),
        example("b", "rust", true)
    );
    Catalog::from_files(decision_with(&one)).unwrap();
    let two = format!(
        "  examples:\n{}{}",
        example("a", "go", true),
        example("b", "go", true)
    );
    rejected(decision_with(&two), "more than one canonical");
    let marked_once = format!(
        "  examples:\n{}{}",
        example("a", "go", true),
        example("b", "go", false)
    );
    Catalog::from_files(decision_with(&marked_once)).unwrap();
}

#[test]
fn example_rules_are_enforced() {
    let file = |extra: &str| {
        format!("  examples:\n    - name: e\n      language: go\n      kind: valid\n{extra}")
    };
    rejected(decision_with(&file("      files: []\n")), "no files");
    rejected(
        decision_with(&file(
            "      files: [{ path: a, body: x }]\n      expect: [{ line: 1 }]\n",
        )),
        "expects no diagnostics",
    );
    rejected(
        decision_with(&file(
            "      files: [{ path: a, body: x }, { path: a, body: y }]\n",
        )),
        "repeated",
    );
    rejected(
        decision_with(&file(
            "      files: [{ path: a, body: x }]\n      options: { nope: 1 }\n",
        )),
        "unknown option",
    );
    rejected(
        decision_with(&file("      files: [{ path: a }]\n")),
        "exactly one of",
    );
    rejected(
        decision_with(&file("      files: [{ path: a, body: x, source: y }]\n")),
        "exactly one of",
    );
}

#[test]
fn example_sources_resolve_next_to_the_decision_file() {
    let example = "  examples:\n    - name: e\n      language: go\n      kind: valid\n      files: [{ path: a.go, source: examples/a.go }]\n";
    rejected(decision_with(example), "does not exist");
    let mut files = decision_with(example);
    files.insert("p/s/examples/a.go".into(), "package a\n".into());
    let catalog = Catalog::from_files(files).unwrap();
    assert_eq!(
        catalog.decision("p/a").unwrap().examples[0].files[0].text(),
        "package a\n"
    );
    rejected(
        decision_with(&example.replace("examples/a.go", "../a.go")),
        "escapes",
    );
}

fn options(kind: &str, default: &str, per_language: &str) -> String {
    format!(
        "  options:\n    type: object\n    properties:\n      max:\n        type: {kind}\n        default: {default}\n        description: d\n    additionalProperties: false\n{per_language}"
    )
}

#[test]
fn option_values_match_their_type() {
    Catalog::from_files(decision_with(&options("integer", "1", ""))).unwrap();
    Catalog::from_files(decision_with(&options("array", "[a]", ""))).unwrap();
    rejected(
        decision_with(&options("integer", "one", "")),
        "not an integer",
    );
    rejected(
        decision_with(&options(
            "integer",
            "1",
            "  languages:\n    go:\n      options: { max: true }\n",
        )),
        "not an integer",
    );
    rejected(
        decision_with(&options(
            "integer",
            "1",
            "  languages:\n    go:\n      options: { mx: 2 }\n",
        )),
        "unknown option `mx`",
    );
    rejected(
        decision_with(&options("integer", "1", "").replace("false", "true")),
        "additionalProperties",
    );
    rejected(
        decision_with(&options("integer", "1", "").replace("description: d", "description: ''")),
        "needs a description",
    );
}

const LIMIT_OPTION: &str = "  options:\n    type: object\n    properties:\n      max:\n        $ref: '#/$defs/limit'\n        default: 3\n        description: d\n    additionalProperties: false\n";

fn limit_decision() -> Decision {
    Catalog::from_files(decision_with(LIMIT_OPTION))
        .unwrap()
        .decision("p/a")
        .unwrap()
        .clone()
}

#[test]
fn a_limit_is_an_integer_or_an_object_of_integers() {
    let decision = limit_decision();
    let max = |v| Map::from_iter([("max".to_owned(), v)]);
    for ok in [
        json!(-1),
        json!(0),
        json!(7),
        json!({}),
        json!({ "default": 6, "constructor": 7, "test": -1 }),
        json!({ "default": -1, "function": 3 }),
    ] {
        assert!(
            decision.resolve_options(&max(ok.clone()), None).is_ok(),
            "{ok}"
        );
    }
    for (bad, needle) in [
        (json!(-2), "at least -1"),
        (json!("3"), "matches none of the allowed shapes"),
        (json!(null), "matches none of the allowed shapes"),
        (json!(1.5), "matches none of the allowed shapes"),
        (json!({ "default": -2 }), "at least -1"),
        (
            json!({ "default": "x" }),
            "matches none of the allowed shapes",
        ),
        (json!({ "closure": 3 }), "unknown key `closure`"),
    ] {
        let problem = decision
            .resolve_options(&max(bad.clone()), None)
            .unwrap_err()
            .to_string();
        assert!(problem.contains(needle), "{bad}: {problem}");
    }
}

#[test]
fn one_of_accepts_exactly_one_matching_shape() {
    let both = "  options:\n    type: object\n    properties:\n      v:\n        oneOf:\n          - { type: number }\n          - { type: integer }\n        default: 1.5\n        description: d\n    additionalProperties: false\n";
    let decision = Catalog::from_files(decision_with(both))
        .unwrap()
        .decision("p/a")
        .unwrap()
        .clone();
    let v = |x| Map::from_iter([("v".to_owned(), x)]);
    assert!(decision.resolve_options(&v(json!(2.5)), None).is_ok());
    let problem = decision
        .resolve_options(&v(json!(2)), None)
        .unwrap_err()
        .to_string();
    assert!(problem.contains("more than one"), "{problem}");
}

#[test]
fn every_decision_is_listed_once_in_the_pack_its_labels_name() {
    let mut files = base();
    files.insert("p/s/b.yaml".into(), decision("p/b", "s", SPEC));
    rejected(
        files,
        "`p/b` names section `s` of pack `p` but its pack does not list it",
    );
    rejected(
        with("p/pack.yaml", &pack("p", &[("s", &["a", "a"])])),
        "listed twice",
    );
    rejected(
        with("p/pack.yaml", &pack("p", &[("s", &["a", "z"])])),
        "`z` is listed in section `s` but is not defined",
    );
    rejected(
        with("p/pack.yaml", &pack("p", &[("s", &["a"]), ("s", &[])])),
        "section `s` is listed twice",
    );
    let mut orphan = base();
    orphan.insert("q/s/x.yaml".into(), decision("q/x", "s", SPEC));
    rejected(orphan, "does not list it");
}

#[test]
fn files_that_are_not_documents_are_ignored() {
    let mut files = base();
    files.insert(
        "p/s/testdata/Cargo.toml".into(),
        "[package]\nname = \"x\"\n".into(),
    );
    files.insert("p/s/notes.md".into(), "# notes\n".into());
    Catalog::from_files(files).unwrap();
}

#[test]
fn the_same_catalog_reads_from_json_and_toml() {
    let json = r#"{"apiVersion":"lighthouse/v1alpha1","kind":"Pack","metadata":{"name":"p"},"spec":{"title":"P","intro":"x","sections":[{"name":"s","title":"S","intro":"x","decisions":["a"]}]}}"#;
    let toml = "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Decision\"\n[metadata]\nname = \"p/a\"\n[metadata.labels]\n\"lighthouse/pack\" = \"p\"\n\"lighthouse/section\" = \"s\"\n[spec]\ntitle = \"A\"\ncontext = \"i\"\nrequirement = \"A MUST b.\"\n[spec.scope]\nsubject = \"file\"\n";
    let mut files = Files::new();
    files.insert("p/pack.json".into(), json.into());
    files.insert("p/s/a.toml".into(), toml.into());
    let catalog = Catalog::from_files(files).unwrap();
    assert_eq!(catalog.decision("p/a").unwrap().title, "A");
}

#[test]
fn several_documents_may_share_one_yaml_file() {
    let mut files = Files::new();
    files.insert(
        "all.yaml".into(),
        format!(
            "{}---\n{}",
            pack("p", &[("s", &["a"])]),
            decision("p/a", "s", SPEC)
        ),
    );
    assert_eq!(Catalog::from_files(files).unwrap().decisions().count(), 1);
}

#[test]
fn sources_map_to_decisions_or_carry_a_reason() {
    let cases = [
        ("- ref: d#a-1\n      text: t\n", false),
        (
            "- ref: d#a-1\n      text: t\n      decisions: [p/zzz]\n",
            false,
        ),
        (
            "- ref: d#a-1\n      text: t\n      decisions: [p/a]\n      omitted: x\n",
            false,
        ),
        ("- ref: d#a-1\n      text: t\n      omitted: ' '\n", false),
        (
            "- ref: d#a-1\n      text: t\n      decisions: [p/a]\n",
            true,
        ),
        (
            "- ref: d#a-1\n      text: t\n      omitted: project-specific\n",
            true,
        ),
    ];
    for (sources, ok) in cases {
        let text = format!(
            "apiVersion: lighthouse/v1alpha1\nkind: SourceMap\nmetadata:\n  name: sources\nspec:\n  sources:\n    {sources}"
        );
        let files = with("sources.yaml", &text);
        assert_eq!(Catalog::from_files(files).is_ok(), ok, "{sources}");
    }
}
