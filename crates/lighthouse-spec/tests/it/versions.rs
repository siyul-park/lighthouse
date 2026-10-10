use lighthouse_model::Severity;
use lighthouse_spec::{
    Catalog, Check, CheckKind, Decision, ExampleFile, ModelCheck, Status, Subject,
};
use lighthouse_test_support::catalog::*;
use serde_json::{Map, json};

fn bundled(id: &str) -> &'static Decision {
    Catalog::bundled()
        .decision(id)
        .unwrap_or_else(|| panic!("missing {id}"))
}

mod write {
    use std::fs;

    use lighthouse_resource::Metadata;
    use lighthouse_spec::{
        BuiltinCheck, Check, CheckKind, DecisionSpec, Example, ExampleKind, Expect, LanguageSpec,
        NamedRule, ObjectType, OptionSchema, OptionType, OptionsSchema, Scope,
    };

    use super::*;

    fn rich() -> Decision {
        let metadata = Metadata {
            name: "p/rich".to_owned(),
            labels: [
                ("lighthouse/pack".to_owned(), "p".to_owned()),
                ("lighthouse/section".to_owned(), "s".to_owned()),
            ]
            .into(),
            annotations: Default::default(),
        };
        let spec = DecisionSpec {
            title: "Rich".into(),
            intent: "To be written.".into(),
            scope: Scope::code(Subject::File),
            requirement: "A rich decision MUST round-trip.".into(),
            status: Status::Accepted,
            supersedes: Vec::new(),
            consequences: Some("More tests.".into()),
            severity: Some(Severity::Warn),
            evidence: vec!["x".into()],
            exceptions: Some("Generated code.".into()),
            options: Some(OptionsSchema {
                kind: ObjectType::Object,
                properties: [(
                    "max".to_owned(),
                    OptionSchema {
                        kind: OptionType::Integer,
                        default: json!(3),
                        description: "Limit.".into(),
                    },
                )]
                .into(),
                additional_properties: false,
            }),
            languages: [(
                "go".to_owned(),
                LanguageSpec {
                    options: Map::from_iter([("max".to_owned(), json!(4))]),
                    tuning: Some("Go wording.".into()),
                },
            )]
            .into(),
            check: Some(Check::of(CheckKind::Builtin(BuiltinCheck::Named(
                NamedRule {
                    id: "p/rich".into(),
                },
            )))),
            fix: None,
            citation: Some("Someone 2001".into()),
            strict: false,
            examples: vec![
                Example {
                    name: "bad".into(),
                    language: "go".into(),
                    kind: ExampleKind::Invalid,
                    files: vec![ExampleFile::inline("a.go", "package a\n\nfunc F() {}")],
                    canonical: false,
                    expect: vec![Expect {
                        line: 3,
                        message: Some("F".into()),
                    }],
                    options: Map::from_iter([("max".to_owned(), json!(1))]),
                    fixed: Vec::new(),
                },
                Example {
                    name: "good".into(),
                    language: "go".into(),
                    kind: ExampleKind::Valid,
                    files: vec![ExampleFile::inline("a.go", "package a")],
                    canonical: false,
                    expect: Vec::new(),
                    options: Map::new(),
                    fixed: Vec::new(),
                },
            ],
        };
        Decision::new(metadata, spec)
    }

    fn write_base(root: &std::path::Path) {
        for (path, text) in base() {
            let target = root.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, text).unwrap();
        }
    }

    #[test]
    fn written_decisions_load_back_unchanged_and_join_the_section_order() {
        let dir = tempfile::tempdir().unwrap();
        write_base(dir.path());
        let decision = rich();
        Catalog::write_decision(dir.path(), &decision).unwrap();
        Catalog::write_decision(dir.path(), &decision).unwrap();

        let catalog = Catalog::load(dir.path()).unwrap();
        assert_eq!(catalog.decision("p/rich"), Some(&decision));
        let ids: Vec<_> = catalog.decisions().map(|d| d.id()).collect();
        assert_eq!(ids, ["p/a", "p/rich"]);
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("p/s"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
        let text = fs::read_to_string(dir.path().join("p/s/rich.yaml")).unwrap();
        assert!(
            text.starts_with("# yaml-language-server: $schema="),
            "{text}"
        );
        assert!(
            text.contains("apiVersion: lighthouse/v1alpha1\nkind: Decision\n"),
            "{text}"
        );
    }

    #[test]
    fn invalid_decisions_and_unknown_sections_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write_base(dir.path());
        let bad = rich().map_spec(|mut spec| {
            spec.intent = String::new();
            spec
        });
        assert!(Catalog::write_decision(dir.path(), &bad).is_err());
        let elsewhere = {
            let original = rich();
            let mut metadata = original.metadata().clone();
            metadata
                .labels
                .insert("lighthouse/section".to_owned(), "nope".to_owned());
            Decision::new(metadata, original.spec().clone())
        };
        assert!(Catalog::write_decision(dir.path(), &elsewhere).is_err());
        assert!(!dir.path().join("p/s/rich.yaml").exists());
        assert_eq!(Catalog::load(dir.path()).unwrap().decisions().count(), 1);
    }
}

#[test]
fn decision_version_changes_with_its_definition() {
    let decision = bundled("core/max-file-lines");
    assert_eq!(decision.version().len(), 16);
    assert_eq!(decision.version(), decision.clone().version());
    let edited = decision.clone().map_spec(|mut spec| {
        spec.requirement.push('!');
        spec
    });
    assert_ne!(edited.version(), decision.version());
}

#[test]
fn catalog_version_changes_with_any_decision() {
    let catalog = Catalog::bundled();
    assert_eq!(catalog.version(), Catalog::bundled().version());
    assert_ne!(catalog.version(), Catalog::default().version());
}

#[test]
fn meaning_version_ignores_wording_examples_tuning_option_descriptions_and_adr_prose() {
    let decision = bundled("design/coupling-signal");
    let reworded = decision.clone().map_spec(|mut spec| {
        spec.intent.push_str(" More prose.");
        spec.examples.clear();
        for language in spec.languages.values_mut() {
            language.tuning = None;
        }
        for property in spec.options.as_mut().unwrap().properties.values_mut() {
            property.description.push_str(" Reworded.");
        }
        spec.requirement = format!("  {}  ", spec.requirement.replace(' ', "  "));
        spec.consequences = Some("Then.".into());
        spec
    });
    assert_eq!(reworded.meaning_version(), decision.meaning_version());
    assert_ne!(reworded.version(), decision.version());
}

#[test]
fn meaning_version_follows_what_the_decision_demands() {
    let decision = bundled("design/coupling-signal");
    let changed = |change: fn(&mut lighthouse_spec::DecisionSpec)| {
        decision
            .clone()
            .map_spec(|mut spec| {
                change(&mut spec);
                spec
            })
            .meaning_version()
    };
    let base = decision.meaning_version();
    assert_ne!(changed(|s| s.requirement.push_str(" Always.")), base);
    assert_ne!(changed(|s| s.severity = Some(Severity::Info)), base);
    assert_ne!(changed(|s| s.scope.subject = Subject::Module), base);
    assert_ne!(
        changed(|s| {
            let property = s
                .options
                .as_mut()
                .unwrap()
                .properties
                .values_mut()
                .next()
                .unwrap();
            property.default = json!(12345);
        }),
        base
    );
    assert_ne!(
        changed(|s| {
            s.languages
                .entry("go".into())
                .or_default()
                .options
                .insert("hub_fan_in".into(), json!(77));
        }),
        base
    );
}

#[test]
fn how_a_decision_is_checked_changes_its_check_revision_and_never_its_meaning() {
    let decision = bundled("design/coupling-signal");
    let unchecked = decision.clone().map_spec(|mut spec| {
        spec.check = None;
        spec
    });
    let judged = decision.clone().map_spec(|mut spec| {
        spec.check = Some(Check::of(CheckKind::Model(ModelCheck::default())));
        spec
    });
    assert_eq!(unchecked.meaning_version(), decision.meaning_version());
    assert_eq!(judged.meaning_version(), decision.meaning_version());
    assert_ne!(unchecked.check_revision(), decision.check_revision());
    assert_ne!(judged.check_revision(), unchecked.check_revision());
    assert_ne!(judged.version(), decision.version());
}

/// What `semantic_version` was in the build before the resource model, for
/// every bundled pattern: recorded verdicts carry these values.
const LEGACY: &str = include_str!("legacy-semantic-versions.tsv");

#[test]
fn migrated_decisions_keep_the_semantic_version_their_verdicts_were_recorded_under() {
    let mut seen = 0;
    for line in LEGACY.lines().filter(|l| !l.starts_with('#')) {
        let (id, version) = line.split_once('\t').unwrap();
        let decision = bundled(id);
        assert_eq!(
            decision.legacy_semantic_version().as_deref(),
            Some(version),
            "{id}"
        );
        seen += 1;
    }
    assert_eq!(seen, Catalog::bundled().decisions().count());
}

#[test]
fn the_legacy_version_moves_with_the_decision_so_a_changed_decision_expires_old_verdicts() {
    let decision = bundled("core/max-file-lines");
    let changed = decision.clone().map_spec(|mut spec| {
        spec.requirement.push_str(" Always.");
        spec
    });
    assert_ne!(
        changed.legacy_semantic_version(),
        decision.legacy_semantic_version()
    );
    let scoped = decision.clone().map_spec(|mut spec| {
        spec.scope.subject = Subject::Symbol;
        spec
    });
    assert_eq!(
        scoped.legacy_semantic_version(),
        decision.legacy_semantic_version(),
        "the scope was never part of what a verdict pinned"
    );
    assert_ne!(scoped.meaning_version(), decision.meaning_version());
}

#[test]
fn a_cel_decision_that_was_never_a_rule_file_has_no_legacy_version() {
    let catalog = Catalog::from_local(files(&[(
        "probe.yaml",
        decision(
            "local/probe",
            "rules",
            "  title: P\n  intent: i\n  scope: { subject: file }\n  requirement: A MUST b.\n  severity: info\n  evidence: [x]\n  check:\n    type: cel\n    select: file\n    where: 'true'\n    message: m\n  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{ path: a.txt, body: x }]\n      expect: [{ line: 1 }]\n    - name: good\n      language: text\n      kind: valid\n      files: [{ path: a.txt, body: x }]\n",
        ),
    )]))
    .unwrap();
    assert_eq!(
        catalog
            .decision("local/probe")
            .unwrap()
            .legacy_semantic_version(),
        None
    );
}
