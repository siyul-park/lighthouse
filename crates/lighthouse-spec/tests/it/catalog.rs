use lighthouse_model::{RunScope, Severity, needs_review};
use lighthouse_spec::{Catalog, Content, Decision, ExampleFile, Subject, authored_severity};
use lighthouse_test_support::catalog::*;
use serde_json::{Map, json};

fn bundled(id: &str) -> &'static Decision {
    Catalog::bundled()
        .decision(id)
        .unwrap_or_else(|| panic!("missing {id}"))
}

#[test]
fn bundled_catalog_has_core_design_and_testing_packs() {
    let ids: Vec<_> = Catalog::bundled().packs.iter().map(|p| &*p.id).collect();
    assert_eq!(ids, ["core", "design", "testing"]);
}

#[test]
fn a_finding_asks_for_review_unless_its_decision_authored_an_error_or_a_judgment_stands() {
    for (authored, asks) in [
        (Severity::Error, false),
        (Severity::Warn, true),
        (Severity::Info, true),
    ] {
        assert_eq!(needs_review(authored, false), asks, "{authored}");
        assert!(!needs_review(authored, true), "{authored} judged");
    }
    let warn = bundled("design/private-helper-callers");
    assert!(needs_review(
        authored_severity(Severity::Error, Some(warn)),
        false
    ));
    let definitive = bundled("design/declaration-groups");
    assert!(!needs_review(
        authored_severity(Severity::Warn, Some(definitive)),
        false
    ));
}

#[test]
fn decision_severity() {
    let mut decision = bundled("design/error-identity").clone();
    assert_eq!(decision.severity(), Some(Severity::Warn));
    decision = decision.map_spec(|spec| lighthouse_spec::DecisionSpec {
        severity: Some(Severity::Error),
        ..spec
    });
    assert_eq!(decision.severity(), Some(Severity::Error));
}

#[test]
fn subject_domain_follows_the_subject_and_decisions_are_judged_over_the_project() {
    use lighthouse_spec::Domain;

    assert_eq!(Subject::Decision.domain(), Domain::Spec);
    assert_eq!(Subject::Decision.run_scope(), RunScope::Project);
    for subject in [
        Subject::Symbol,
        Subject::File,
        Subject::Module,
        Subject::Test,
    ] {
        assert_eq!(subject.domain(), Domain::Code, "{subject}");
    }
}

#[test]
fn subject_run_scope() {
    let cases = [
        (Subject::Symbol, RunScope::File),
        (Subject::File, RunScope::File),
        (Subject::Test, RunScope::File),
        (Subject::Module, RunScope::Project),
        (Subject::Project, RunScope::Project),
    ];
    for (subject, want) in cases {
        assert_eq!(subject.run_scope(), want, "{subject}");
    }
}

#[test]
fn decision_resolve_options() {
    let decision = bundled("core/max-lines").clone().map_spec(|mut spec| {
        spec.languages
            .entry("go".to_owned())
            .or_default()
            .options
            .insert("max".to_owned(), json!(500));
        spec
    });
    let none = Map::new();
    assert_eq!(decision.resolve_options(&none, None).unwrap()["max"], 1000);
    assert_eq!(
        decision.resolve_options(&none, Some("go")).unwrap()["max"],
        500
    );
    assert_eq!(
        decision.resolve_options(&none, Some("rust")).unwrap()["max"],
        1000
    );

    let set = |v| Map::from_iter([("max".to_owned(), v)]);
    assert_eq!(
        decision
            .resolve_options(&set(json!(7)), Some("go"))
            .unwrap()["max"],
        7
    );
    let unknown = Map::from_iter([("mx".to_owned(), json!(1))]);
    assert!(
        decision
            .resolve_options(&unknown, None)
            .unwrap_err()
            .to_string()
            .contains("unknown option")
    );
    assert!(decision.resolve_options(&set(json!("big")), None).is_err());
}

#[test]
fn example_file_text() {
    let files = &bundled("design/error-identity").examples[0].files;
    assert!(matches!(files[0].content, Content::File(_)));
    assert!(files[0].text().contains("fmt.Errorf"));
    let inline = &bundled("design/no-single-use-wrapper").examples[0].files[0];
    assert!(matches!(inline.content, Content::Inline(_)));
    assert_eq!(ExampleFile::inline("a.go", "package a").text(), "package a");
}

#[test]
fn checked_decisions_carry_runnable_examples() {
    for decision in Catalog::bundled().decisions().filter(|d| d.automated()) {
        let invalid = decision.examples.iter().find(|e| !e.expect.is_empty());
        assert!(
            invalid.is_some(),
            "{} has no invalid example with expectations",
            decision.id()
        );
    }
}

#[test]
fn a_cel_check_lives_in_its_decision() {
    let rule = decision_with("");
    let probe = "  title: P
  context: i
  scope: { subject: symbol }
  requirement: A MUST b.
  severity: error
  check:
    type: cel
    select: symbol
    where: 'true'
    message: x
  examples:
    - name: bad
      language: text
      kind: invalid
      files: [{ path: a.txt, body: x }]
      expect: [{ line: 1 }]
    - name: good
      language: text
      kind: valid
      files: [{ path: a.txt, body: x }]
";
    drop(rule);
    let files = files(&[("probe.yaml", decision("local/probe", "rules", probe))]);
    let catalog = Catalog::from_local(files).unwrap();
    let decision = catalog.decision("local/probe").unwrap();
    assert!(matches!(
        decision.check,
        Some(lighthouse_spec::Check {
            kind: lighthouse_spec::CheckKind::Cel(_),
            ..
        })
    ));
    assert_eq!(decision.pack(), "local");
    assert_eq!(decision.section(), "rules");
}

#[test]
fn every_bundled_decision_has_its_own_uid() {
    let mut seen = std::collections::BTreeSet::new();
    for decision in Catalog::bundled().decisions() {
        let uid = decision
            .uid()
            .unwrap_or_else(|| panic!("{} has no uid", decision.id()));
        assert!(lighthouse_resource::is_uid(uid), "{}: {uid}", decision.id());
        assert!(seen.insert(uid), "{} repeats a uid", decision.id());
    }
}

mod validation {
    use super::*;

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
        let wrong_pack =
            decision("p/a", "s", SPEC).replace("lighthouse/pack: p", "lighthouse/pack: q");
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
            decision_with(
                &options("integer", "1", "").replace("description: d", "description: ''"),
            ),
            "needs a description",
        );
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
}

mod overlay {
    use super::*;

    const OPTION: &str = "  options:\n    type: object\n    properties:\n      max:\n        type: integer\n        default: 1\n        description: d\n    additionalProperties: false\n";

    fn base_catalog() -> Catalog {
        let judged = format!("{SPEC}  severity: info\n  check:\n    type: model\n{OPTION}");
        Catalog::from_files(with("p/s/a.yaml", &decision("p/a", "s", &judged))).unwrap()
    }

    fn local(entries: &[(&str, String)]) -> Catalog {
        Catalog::from_files(files(entries)).unwrap()
    }

    #[test]
    fn local_layer_adds_packs_sections_and_decisions() {
        let extra = local(&[
            (
                "p/pack.yaml",
                pack("p", &[("s", &["b"]), ("t", &[])]).replace("title: P", "title: Ignored"),
            ),
            ("p/s/b.yaml", decision("p/b", "s", SPEC)),
            ("q/pack.yaml", pack("q", &[])),
        ]);
        let merged = Catalog::overlay(&base_catalog(), &extra).unwrap();
        let ids: Vec<_> = merged.decisions().map(|d| d.id()).collect();
        assert_eq!(ids, ["p/a", "p/b"]);
        assert_eq!(merged.packs[0].title, "P");
        assert_eq!(merged.packs[0].sections.len(), 2);
        assert_eq!(merged.packs.len(), 2);
    }

    #[test]
    fn colliding_ids_are_rejected() {
        let clash = local(&[
            ("p/pack.yaml", pack("p", &[("s", &["a"])])),
            ("p/s/a.yaml", decision("p/a", "s", SPEC)),
        ]);
        let err = Catalog::overlay(&base_catalog(), &clash).unwrap_err();
        assert!(err.to_string().contains("defined twice"));
    }

    fn team(name: &str, spec: &str) -> String {
        format!(
            "apiVersion: lighthouse/v1alpha1\nkind: Project\nmetadata:\n  name: {name}\nspec:\n{spec}"
        )
    }

    #[test]
    fn a_project_document_of_a_layer_is_a_project_that_extends_can_name() {
        let layer = Catalog::from_local(files(&[(
            "team.yaml",
            team("team/base", "  rules:\n    p/a: warn\n"),
        )]))
        .unwrap();
        let merged = Catalog::overlay(&base_catalog(), &layer).unwrap();

        let projects = merged.projects().unwrap();

        let names: Vec<&str> = projects.names().collect();
        assert!(names.contains(&"team/base"), "{names:?}");
        assert!(projects.get("team/base").is_some());
    }

    #[test]
    fn a_project_that_extends_an_unknown_project_or_itself_is_refused() {
        let unknown = Catalog::from_local(files(&[(
            "team.yaml",
            team("team/a", "  extends: [team/none]\n"),
        )]))
        .unwrap();
        let err = Catalog::overlay(&base_catalog(), &unknown).unwrap_err();
        assert!(
            err.to_string().contains("unknown project `team/none`"),
            "{err}"
        );
        let circle = Catalog::from_local(files(&[
            ("a.yaml", team("team/a", "  extends: [team/b]\n")),
            ("b.yaml", team("team/b", "  extends: [team/a]\n")),
        ]))
        .unwrap();
        let err = Catalog::overlay(&base_catalog(), &circle).unwrap_err();
        assert!(err.to_string().contains("extends itself"), "{err}");
    }

    #[test]
    fn layers_validate_their_own_sources_only() {
        let sources = "apiVersion: lighthouse/v1alpha1\nkind: SourceMap\nmetadata:\n  name: sources\nspec:\n  sources:\n    - ref: d#x-1\n      text: t\n      omitted: project-specific\n";
        let layer = local(&[
            ("l/pack.yaml", pack("l", &[])),
            ("sources.yaml", sources.to_owned()),
        ]);
        let merged = Catalog::overlay(&base_catalog(), &layer).unwrap();
        assert!(merged.decisions().count() == 1);
    }

    #[test]
    fn a_local_decision_needs_the_local_prefix_and_a_cel_check() {
        let probe = decision("p/x", "rules", SPEC);
        let error = Catalog::from_local(files(&[("x.yaml", probe)])).unwrap_err();
        assert!(error.to_string().contains("local/<name>"), "{error}");
        let builtin = decision(
            "local/x",
            "rules",
            &format!("{SPEC}{CHECKED_SPEC}  check:\n    type: builtin\n    id: local/x\n"),
        );
        let error = Catalog::from_local(files(&[("x.yaml", builtin)])).unwrap_err();
        assert!(error.to_string().contains("standard operation"), "{error}");
    }
}

#[test]
fn the_authored_severity_follows_the_decision_else_what_was_reported() {
    let warn = bundled("design/private-helper-callers");
    assert_eq!(
        authored_severity(Severity::Error, Some(warn)),
        Severity::Info
    );
    assert_eq!(authored_severity(Severity::Error, None), Severity::Error);
    assert_eq!(authored_severity(Severity::Warn, None), Severity::Warn);
    assert_eq!(authored_severity(Severity::Info, None), Severity::Info);
}

#[test]
fn checked_decisions_mark_one_canonical_example_per_language() {
    for decision in Catalog::bundled().decisions() {
        for language in ["go", "rust"] {
            let valid = |e: &&lighthouse_spec::Example| {
                e.language == language && e.kind == lighthouse_spec::ExampleKind::Valid
            };
            if !decision.automated() || !decision.examples.iter().any(|e| valid(&e)) {
                continue;
            }
            let marked = decision
                .examples
                .iter()
                .filter(valid)
                .filter(|e| e.canonical);
            assert_eq!(marked.count(), 1, "{} {language}", decision.id());
        }
    }
}

#[test]
fn decision_text_and_write_local_round_trip_through_the_local_layer() {
    let spec = "  title: Probe\n  context: A probe.\n  scope: { subject: file }\n  requirement: A probe MUST hold.\n  severity: error\n  check:\n    type: cel\n    select: file\n    where: 'true'\n    message: m\n  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{ path: a.txt, body: x }]\n      expect: [{ line: 1 }]\n    - name: good\n      language: text\n      kind: valid\n      files: [{ path: a.txt, body: x }]\n";
    let layer = Catalog::from_local(files(&[(
        "probe.yaml",
        decision("local/probe", "rules", spec),
    )]))
    .unwrap();
    let probe = layer.decision("local/probe").unwrap();
    let text = Catalog::decision_text(probe).unwrap();
    assert!(text.contains("kind: Decision"), "{text}");
    assert!(text.contains("type: cel"), "{text}");

    let dir = tempfile::tempdir().unwrap();
    let rules = dir.path().join("nested/decisions");
    Catalog::write_local(&rules, "probe", &text).unwrap();
    let written = std::fs::read_to_string(rules.join("probe.yaml")).unwrap();
    assert_eq!(written, text);
    assert!(!rules.join("probe.yaml.tmp").exists());

    let layer = Catalog::from_local([("probe.yaml".to_owned(), written)].into()).unwrap();
    assert_eq!(layer.decision("local/probe"), Some(probe));
    assert!(Catalog::write_local(&rules.join("probe.yaml/x"), "y", "z").is_err());
}

#[test]
fn write_local_refuses_names_and_targets_that_leave_the_directory() {
    for name in [
        "", "../evil", "a/b", "a\\b", "..", "a..b", ".hidden", "Upper", "x y",
    ] {
        assert!(!Catalog::local_name_ok(name), "{name:?}");
    }
    for name in ["short-notes", "a.b_c-1", "9lives"] {
        assert!(Catalog::local_name_ok(name), "{name:?}");
    }

    let dir = tempfile::tempdir().unwrap();
    let rules = dir.path().join("decisions");
    for name in ["../evil", "a/b", ""] {
        assert!(Catalog::write_local(&rules, name, "x").is_err(), "{name:?}");
    }
    assert!(!dir.path().join("evil.yaml").exists());

    std::fs::create_dir_all(&rules).unwrap();
    let outside = dir.path().join("elsewhere.yaml");
    std::fs::write(&outside, "keep").unwrap();
    std::os::unix::fs::symlink(&outside, rules.join("linked.yaml")).unwrap();
    assert!(Catalog::write_local(&rules, "linked", "overwritten").is_err());
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
}

#[test]
fn write_atomic_replaces_a_file_and_leaves_no_temporary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.json");
    lighthouse_spec::write_atomic(&path, "one").unwrap();
    lighthouse_spec::write_atomic(&path, "two").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
    let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(names.len(), 1);
}

#[test]
fn write_atomic_guarded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "old").unwrap();

    let refused =
        lighthouse_spec::write_atomic_guarded(&path, "new", &|| Err("changed".to_owned()));
    let allowed = lighthouse_spec::write_atomic_guarded(&path, "new", &|| Ok(()));

    assert!(refused.unwrap_err().to_string().contains("changed"));
    allowed.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    let leftovers = std::fs::read_dir(dir.path()).unwrap().count();
    assert_eq!(leftovers, 1, "a refused write leaves no temporary file");
}

#[test]
fn decision_strict() {
    assert!(bundled("design/private-helper-callers").strict());
    assert!(!bundled("design/exported-doc").strict());
}

#[test]
fn scope_applicability() {
    let scope = bundled("testing/external-package").scope;
    assert_eq!(
        scope.applicability().tests,
        lighthouse_model::TestScope::Only
    );
    assert!(!scope.applicability().generated);
    let all = bundled("core/max-lines").scope.applicability();
    assert!(all.generated);
    assert_eq!(all.tests, lighthouse_model::TestScope::Include);
}
