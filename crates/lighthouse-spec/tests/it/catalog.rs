use lighthouse_model::{RunScope, Severity};
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
        assert_eq!(authored.needs_review(false), asks, "{authored}");
        assert!(!authored.needs_review(true), "{authored} judged");
    }
    let warn = bundled("design/private-helper-callers");
    assert!(authored_severity(Severity::Error, Some(warn)).needs_review(false));
    let definitive = bundled("design/declaration-groups");
    assert!(!authored_severity(Severity::Warn, Some(definitive)).needs_review(false));
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
        (Subject::Edge, RunScope::Project),
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
fn decision_resolve_options_checks_the_shape_of_lists_and_objects() {
    let decision = bundled("design/layers");
    let options = |ignore| Map::from_iter([("ignore".to_owned(), ignore)]);
    let ok = json!([{ "from": "a", "to": "b", "reason": "moves down" }]);
    assert!(decision.resolve_options(&options(ok), None).is_ok());
    let missing = json!([{ "from": "a", "to": "b" }]);
    let problem = decision
        .resolve_options(&options(missing), None)
        .unwrap_err()
        .to_string();
    assert!(problem.contains("needs `reason`"), "{problem}");
    let wrong = json!([{ "from": "a", "to": 1, "reason": "r" }]);
    assert!(decision.resolve_options(&options(wrong), None).is_err());
    let extra = json!([{ "from": "a", "to": "b", "reason": "r", "why": "x" }]);
    assert!(decision.resolve_options(&options(extra), None).is_err());
    let layers = Map::from_iter([("layers".to_owned(), json!([["a"], "b"]))]);
    assert!(decision.resolve_options(&layers, None).is_err());
}

#[test]
fn shape() {
    let shape: lighthouse_spec::Shape = serde_json::from_value(json!({
        "type": "array",
        "items": { "type": "string" },
    }))
    .unwrap();
    assert_eq!(shape.kind, Some(lighthouse_spec::OptionType::Array));
    assert_eq!(
        shape.items.unwrap().kind,
        Some(lighthouse_spec::OptionType::String)
    );
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

#[test]
fn a_document_without_an_envelope_is_a_document_error() {
    let error = Catalog::from_files(with("p/s/a.yaml", "id: p/a\n")).unwrap_err();
    assert!(
        matches!(error, lighthouse_spec::Error::Document(_)),
        "{error}"
    );
}

#[test]
fn with_uid_gives_a_decision_the_identity_it_keeps_across_renames() {
    let decision = bundled("core/allow-annotation").clone();
    let uid = "5d6b1c1e-2b0e-4a43-9a3e-0f1b6f5d2a11";
    assert_eq!(decision.with_uid(uid).uid(), Some(uid));
}

#[test]
fn an_object_limit_merges_over_the_default_and_the_language() {
    let decision = bundled("design/max-params");
    let none = Map::new();
    let go = decision.resolve_options(&none, Some("go")).unwrap();
    assert_eq!(go["max"]["default"], 8);
    assert_eq!(go["max"]["constructor"], 9);
    assert_eq!(go["max"]["test"], -1);
    let rust = decision.resolve_options(&none, Some("rust")).unwrap();
    assert_eq!(rust["max"]["default"], 7);
    assert_eq!(rust["max"]["constructor"], 8);
    assert_eq!(rust["max"]["entrypoint"], -1, "kept from the default");

    let partial = Map::from_iter([("max".to_owned(), json!({ "default": 5 }))]);
    let merged = decision.resolve_options(&partial, Some("rust")).unwrap();
    assert_eq!(merged["max"]["default"], 5);
    assert_eq!(merged["max"]["constructor"], 8, "the language's role stays");

    let whole = Map::from_iter([("max".to_owned(), json!(3))]);
    let single = decision.resolve_options(&whole, None).unwrap();
    assert_eq!(single["max"], 3, "an integer replaces the object");
    let off = Map::from_iter([("max".to_owned(), json!(-1))]);
    assert_eq!(decision.resolve_options(&off, None).unwrap()["max"], -1);
}

#[test]
fn an_option_schema_refuses_keys_it_does_not_know() {
    let text = "type: integer\ndefault: 1\ndescription: d\nminmum: 0\n";
    let parsed: Result<lighthouse_spec::OptionSchema, _> = serde_norway::from_str(text);
    assert!(parsed.is_err(), "a misspelled keyword is refused");
}
