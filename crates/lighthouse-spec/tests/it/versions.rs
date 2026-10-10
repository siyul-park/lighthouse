use lighthouse_model::Severity;
use lighthouse_spec::{
    Catalog, Check, CheckKind, Decision, DecisionStatus, ExampleFile, ModelCheck, Provenance,
    Subject,
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
            uid: None,
            labels: [
                ("lighthouse/pack".to_owned(), "p".to_owned()),
                ("lighthouse/section".to_owned(), "s".to_owned()),
            ]
            .into(),
            annotations: Default::default(),
        };
        let spec = DecisionSpec {
            title: "Rich".into(),
            context: "To be written.".into(),
            scope: Scope::code(Subject::File),
            requirement: "A rich decision MUST round-trip.".into(),
            status: DecisionStatus::Accepted,
            supersedes: Vec::new(),
            consequences: Some("More tests.".into()),
            severity: Some(Severity::Warn),
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
                },
            )]
            .into(),
            check: Some(Check::of(CheckKind::Builtin(BuiltinCheck::Named(
                NamedRule {
                    id: "p/rich".into(),
                },
            )))),
            fix: None,
            provenance: Provenance {
                was_derived_from: vec!["Someone 2001".into()],
            },
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
            spec.context = String::new();
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
    let decision = bundled("core/max-lines");
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
fn meaning_version_ignores_wording_examples_option_descriptions_and_adr_prose() {
    let decision = bundled("design/coupling");
    let reworded = decision.clone().map_spec(|mut spec| {
        spec.context.push_str(" More prose.");
        spec.examples.clear();
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
    let decision = bundled("design/coupling");
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
                .insert("hubFanIn".into(), json!(77));
        }),
        base
    );
}

#[test]
fn how_a_decision_is_checked_changes_its_check_revision_and_never_its_meaning() {
    let decision = bundled("design/coupling");
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
