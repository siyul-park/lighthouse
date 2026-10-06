use lighthouse_model::Severity;
use lighthouse_plugin::Scope as RunScope;
use lighthouse_spec::{Catalog, Content, Enforcement, Error, ExampleFile, Pattern, Scope};
use serde_json::{Map, json};

fn bundled(id: &str) -> &'static Pattern {
    Catalog::bundled()
        .pattern(id)
        .unwrap_or_else(|| panic!("missing {id}"))
}

#[test]
fn bundled_catalog_has_core_design_and_testing_packs() {
    let ids: Vec<_> = Catalog::bundled().packs.iter().map(|p| &*p.id).collect();
    assert_eq!(ids, ["core", "design", "testing"]);
}

#[test]
fn enforcement_default_severity() {
    let cases = [
        (Enforcement::Mechanical, Some(Severity::Error)),
        (Enforcement::Heuristic, Some(Severity::Warn)),
        (Enforcement::Judgment, Some(Severity::Review)),
        (Enforcement::Doc, None),
    ];
    for (enforcement, want) in cases {
        let enforcement: Enforcement = enforcement;
        assert_eq!(enforcement.default_severity(), want, "{enforcement}");
    }
}

#[test]
fn pattern_severity() {
    let mut pattern: Pattern = bundled("design/error-identity").clone();
    assert_eq!(pattern.severity(), Some(Severity::Warn));
    pattern.severity_override = Some(Severity::Error);
    assert_eq!(pattern.severity(), Some(Severity::Error));
}

#[test]
fn scope_rule_scope() {
    let cases = [
        (Scope::Symbol, RunScope::File),
        (Scope::File, RunScope::File),
        (Scope::Test, RunScope::File),
        (Scope::Module, RunScope::Project),
        (Scope::Project, RunScope::Project),
    ];
    for (scope, want) in cases {
        let scope: Scope = scope;
        assert_eq!(scope.rule_scope(), want, "{scope}");
    }
}

#[test]
fn pattern_rule_meta() {
    let meta = bundled("core/max-file-lines").rule_meta().unwrap();
    assert_eq!(meta.id, "core/max-file-lines");
    assert_eq!(meta.severity, Severity::Warn);
    assert_eq!(meta.scope, RunScope::File);
    assert!(
        bundled("design/no-private-types-in-public-api")
            .rule_meta()
            .is_none()
    );
    assert!(bundled("design/signals-are-advisory").rule_meta().is_none());
}

#[test]
fn pattern_resolve_options() {
    let mut pattern: Pattern = bundled("core/max-file-lines").clone();
    pattern
        .options
        .get_mut("max")
        .unwrap()
        .per_language
        .insert("go".into(), json!(500));
    let none = Map::new();
    assert_eq!(pattern.resolve_options(&none, None).unwrap()["max"], 1000);
    assert_eq!(
        pattern.resolve_options(&none, Some("go")).unwrap()["max"],
        500
    );
    assert_eq!(
        pattern.resolve_options(&none, Some("rust")).unwrap()["max"],
        1000
    );

    let set = |v| Map::from_iter([("max".to_owned(), v)]);
    assert_eq!(
        pattern.resolve_options(&set(json!(7)), Some("go")).unwrap()["max"],
        7
    );
    let unknown = Map::from_iter([("mx".to_owned(), json!(1))]);
    assert!(
        pattern
            .resolve_options(&unknown, None)
            .unwrap_err()
            .to_string()
            .contains("unknown option")
    );
    assert!(pattern.resolve_options(&set(json!("big")), None).is_err());
}

#[test]
fn example_file_text() {
    let files = &bundled("design/error-identity").examples[0].files;
    assert!(matches!(files[0].content, Content::File(_)));
    assert!(files[0].text().contains("fmt.Errorf"));
    let inline: &ExampleFile = &bundled("design/single-use-wrapper").examples[0].files[0];
    assert!(matches!(inline.content, Content::Inline(_)));
    assert_eq!(ExampleFile::inline("a.go", "package a").text(), "package a");
}

#[test]
fn implemented_patterns_carry_runnable_examples() {
    for pattern in Catalog::bundled()
        .patterns()
        .filter(|p| p.implementation.is_some())
    {
        let invalid = pattern.examples.iter().find(|e| !e.expect.is_empty());
        assert!(
            invalid.is_some(),
            "{} has no invalid example with expectations",
            pattern.id
        );
    }
}

#[test]
fn catalog_declarative() {
    let rule = "id: local/probe\ntitle: P\nintent: i\nscope: symbol\nrequirement: A MUST b.\nenforcement: mechanical\nevidence: [x]\nexamples:\n  - name: bad\n    language: text\n    kind: invalid\n    files: [{ path: a.txt, body: x }]\n    expect: [{ line: 1 }]\n  - name: good\n    language: text\n    kind: valid\n    files: [{ path: a.txt, body: x }]\nrule:\n  select: symbol\n  where: 'true'\n  message: x\n";
    let files = std::collections::BTreeMap::from([("probe.yaml".to_owned(), rule.to_owned())]);
    let catalog: Catalog = Catalog::from_local(files).unwrap();
    assert!(
        catalog
            .declarative("probe.yaml")
            .unwrap()
            .contains("select: symbol")
    );
    assert_eq!(catalog.declarative("absent.yaml"), None);
    assert_eq!(Catalog::bundled().declarative("absent.yaml"), None);
}

mod fixture {
    use std::collections::BTreeMap;

    pub const PATTERN: &str = "id: p/a\ntitle: A\nintent: i\nscope: file\nrequirement: A MUST b.\nenforcement: mechanical\nevidence: [x]\n";

    pub fn base() -> BTreeMap<String, String> {
        [
            ("p/pack.yaml", "id: p\ntitle: P\nintro: x\nsections: [s]\n"),
            (
                "p/s/section.yaml",
                "id: s\ntitle: S\nintro: x\npatterns: [a]\n",
            ),
            ("p/s/a.yaml", PATTERN),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    pub fn with(path: &str, text: &str) -> BTreeMap<String, String> {
        let mut files = base();
        files.insert(path.to_owned(), text.to_owned());
        files
    }

    pub fn pattern_with(extra: &str) -> BTreeMap<String, String> {
        with("p/s/a.yaml", &format!("{}{extra}", PATTERN))
    }
}

fn rejected(files: std::collections::BTreeMap<String, String>, needle: &str) {
    let err = Catalog::from_files(files).unwrap_err();
    assert!(
        err.to_string().contains(needle),
        "{err} should mention {needle}"
    );
}

mod validation {
    use super::{fixture::*, *};

    const EXAMPLE_PAIR: &str = "examples:
  - name: bad
    language: go
    kind: invalid
    files: [{ path: a.go, body: x }]
  - name: good
    language: go
    kind: valid
    files: [{ path: a.go, body: y }]
";

    #[test]
    fn minimal_catalog_loads() {
        let catalog = Catalog::from_files(base()).unwrap();
        assert_eq!(
            catalog.pattern("p/a").unwrap().severity(),
            Some(Severity::Error)
        );
    }

    #[test]
    fn id_must_match_pack_and_file_name() {
        let files = with("p/s/a.yaml", &PATTERN.replace("p/a", "q/a"));
        assert!(matches!(
            Catalog::from_files(files),
            Err(Error::Layout { .. })
        ));
        let files = with("p/s/a.yaml", &PATTERN.replace("p/a", "p/b"));
        assert!(matches!(
            Catalog::from_files(files),
            Err(Error::Layout { .. })
        ));
    }

    #[test]
    fn ids_must_be_unique_across_sections() {
        let mut files = base();
        files.insert(
            "p/pack.yaml".into(),
            "id: p\ntitle: P\nintro: x\nsections: [s, t]\n".into(),
        );
        files.insert(
            "p/t/section.yaml".into(),
            "id: t\ntitle: T\nintro: x\npatterns: [a]\n".into(),
        );
        files.insert("p/t/a.yaml".into(), PATTERN.into());
        rejected(files, "defined twice");
    }

    #[test]
    fn pattern_rules_are_enforced() {
        rejected(
            with(
                "p/s/a.yaml",
                &PATTERN
                    .replace("mechanical", "doc")
                    .replace("scope", "severity: warn\nscope"),
            ),
            "no severity",
        );
        rejected(
            with("p/s/a.yaml", &PATTERN.replace("MUST", "must")),
            "MUST, SHOULD or MAY",
        );
        rejected(
            with("p/s/a.yaml", &PATTERN.replace("intent: i", "intent: ' '")),
            "intent",
        );
        rejected(
            with("p/s/a.yaml", &PATTERN.replace("evidence: [x]\n", "")),
            "evidence",
        );
        rejected(
            with(
                "p/s/a.yaml",
                &format!(
                    "{}implementation:\n  builtin: p/a\n",
                    PATTERN.replace("mechanical", "doc")
                ),
            ),
            "no implementation",
        );
    }

    #[test]
    fn implemented_checkers_need_a_valid_and_an_invalid_example() {
        let implemented = "implementation:\n  builtin: p/a\n";
        rejected(pattern_with(implemented), "valid and an invalid example");
        Catalog::from_files(pattern_with(&format!("{implemented}{EXAMPLE_PAIR}"))).unwrap();
        let judgment = PATTERN.replace("mechanical", "judgment");
        Catalog::from_files(with("p/s/a.yaml", &format!("{judgment}{implemented}"))).unwrap();
        rejected(
            pattern_with("implementation:\n  declarative: /abs.yaml\n"),
            "relative .yaml path",
        );
    }

    #[test]
    fn example_rules_are_enforced() {
        let file = |extra: &str| {
            format!("examples:\n  - name: e\n    language: go\n    kind: valid\n{extra}")
        };
        rejected(pattern_with(&file("    files: []\n")), "no files");
        rejected(
            pattern_with(&file(
                "    files: [{ path: a, body: x }]\n    expect: [{ line: 1 }]\n",
            )),
            "expects no diagnostics",
        );
        rejected(
            pattern_with(&file(
                "    files: [{ path: a, body: x }, { path: a, body: y }]\n",
            )),
            "repeated",
        );
        rejected(
            pattern_with(&file(
                "    files: [{ path: a, body: x }]\n    options: { nope: 1 }\n",
            )),
            "unknown option",
        );
        rejected(
            pattern_with(&file("    files: [{ path: a }]\n")),
            "exactly one of",
        );
        rejected(
            pattern_with(&file("    files: [{ path: a, body: x, source: y }]\n")),
            "exactly one of",
        );
    }

    #[test]
    fn example_sources_resolve_inside_the_section_directory() {
        let example = "examples:\n  - name: e\n    language: go\n    kind: valid\n    files: [{ path: a.go, source: examples/a.go }]\n";
        rejected(pattern_with(example), "does not exist");
        let mut files = pattern_with(example);
        files.insert("p/s/examples/a.go".into(), "package a\n".into());
        let catalog = Catalog::from_files(files).unwrap();
        assert_eq!(
            catalog.pattern("p/a").unwrap().examples[0].files[0].text(),
            "package a\n"
        );
        rejected(
            pattern_with(&example.replace("examples/a.go", "../a.go")),
            "escapes",
        );
    }

    #[test]
    fn option_values_match_their_type() {
        let option = |ty: &str, default: &str| {
            format!(
                "options:\n  max:\n    type: {ty}\n    default: {default}\n    description: d\n"
            )
        };
        Catalog::from_files(pattern_with(&option("int", "1"))).unwrap();
        Catalog::from_files(pattern_with(&option("list", "[a]"))).unwrap();
        rejected(pattern_with(&option("int", "one")), "not int");
        rejected(
            pattern_with(&format!(
                "{}    per_language: {{ go: true }}\n",
                option("int", "1")
            )),
            "not int",
        );
    }

    #[test]
    fn every_pattern_and_section_is_listed_once() {
        let mut files = base();
        files.insert("p/s/b.yaml".into(), PATTERN.replace("p/a", "p/b"));
        rejected(files, "`b` exists but is not listed");
        rejected(
            with(
                "p/s/section.yaml",
                "id: s\ntitle: S\nintro: x\npatterns: [a, a]\n",
            ),
            "listed twice",
        );
        rejected(
            with(
                "p/s/section.yaml",
                "id: s\ntitle: S\nintro: x\npatterns: [a, z]\n",
            ),
            "`z` is listed but has no file",
        );
        let mut sections = base();
        sections.insert(
            "p/t/section.yaml".into(),
            "id: t\ntitle: T\nintro: x\npatterns: []\n".into(),
        );
        rejected(sections, "`t` exists but is not listed");
    }

    #[test]
    fn nested_example_directories_are_not_patterns() {
        let mut files = base();
        files.insert("p/s/examples/other.yaml".into(), "not: a pattern\n".into());
        Catalog::from_files(files).unwrap();
    }

    #[test]
    fn sources_map_to_patterns_or_carry_a_reason() {
        let cases = [
            ("- ref: d#a-1\n  text: t\n", false),
            ("- ref: d#a-1\n  text: t\n  patterns: [p/zzz]\n", false),
            (
                "- ref: d#a-1\n  text: t\n  patterns: [p/a]\n  omitted: x\n",
                false,
            ),
            ("- ref: d#a-1\n  text: t\n  omitted: ' '\n", false),
            ("- ref: d#a-1\n  text: t\n  patterns: [p/a]\n", true),
            (
                "- ref: d#a-1\n  text: t\n  omitted: project-specific\n",
                true,
            ),
        ];
        for (sources, ok) in cases {
            let files = with("sources.yaml", sources);
            assert_eq!(Catalog::from_files(files).is_ok(), ok, "{sources}");
        }
    }
}

mod overlay {
    use super::{fixture::*, *};

    const OPTION: &str = "options:\n  max:\n    type: int\n    default: 1\n    description: d\n";

    fn local(files: &[(&str, &str)]) -> Catalog {
        let map = files
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Catalog::from_files(map).unwrap()
    }

    fn base_catalog() -> Catalog {
        Catalog::from_files(pattern_with(OPTION)).unwrap()
    }

    const OVERRIDE_LAYOUT: [(&str, &str); 2] = [
        ("l/pack.yaml", "id: l\ntitle: L\nintro: x\nsections: [s]\n"),
        (
            "l/s/section.yaml",
            "id: s\ntitle: S\nintro: x\npatterns: [tweak]\n",
        ),
    ];

    fn tweak(text: &str) -> Catalog {
        let [a, b] = OVERRIDE_LAYOUT;
        local(&[a, b, ("l/s/tweak.yaml", text)])
    }

    #[test]
    fn local_layer_adds_packs_sections_and_patterns() {
        let extra = local(&[
            (
                "p/pack.yaml",
                "id: p\ntitle: Ignored\nintro: x\nsections: [s, t]\n",
            ),
            (
                "p/s/section.yaml",
                "id: s\ntitle: Ignored\nintro: x\npatterns: [b]\n",
            ),
            ("p/s/b.yaml", &PATTERN.replace("p/a", "p/b")),
            (
                "p/t/section.yaml",
                "id: t\ntitle: T\nintro: x\npatterns: []\n",
            ),
            ("q/pack.yaml", "id: q\ntitle: Q\nintro: x\nsections: []\n"),
        ]);
        let merged = Catalog::overlay(&base_catalog(), &extra).unwrap();
        let ids: Vec<_> = merged.patterns().map(|p| &*p.id).collect();
        assert_eq!(ids, ["p/a", "p/b"]);
        assert_eq!(merged.packs[0].title, "P");
        assert_eq!(merged.packs[0].sections.len(), 2);
        assert_eq!(merged.packs.len(), 2);
    }

    #[test]
    fn colliding_ids_are_rejected() {
        let clash = local(&[
            ("p/pack.yaml", "id: p\ntitle: P\nintro: x\nsections: [s]\n"),
            (
                "p/s/section.yaml",
                "id: s\ntitle: S\nintro: x\npatterns: [a]\n",
            ),
            ("p/s/a.yaml", PATTERN),
        ]);
        let err = Catalog::overlay(&base_catalog(), &clash).unwrap_err();
        assert!(err.to_string().contains("defined twice"));
    }

    #[test]
    fn extends_adjusts_only_the_overridable_fields() {
        let patch = tweak(
            "extends: p/a
severity: warn
exceptions: Vendored code.
tuning:
  go: Local wording.
options:
  max:
    default: 9
    per_language: { go: 5 }
examples:
  - name: local
    language: go
    kind: valid
    files: [{ path: a.go, body: z }]
",
        );
        let merged = Catalog::overlay(&base_catalog(), &patch).unwrap();
        let pattern = merged.pattern("p/a").unwrap();
        assert_eq!(pattern.severity(), Some(Severity::Warn));
        assert_eq!(pattern.exceptions.as_deref(), Some("Vendored code."));
        assert_eq!(pattern.tuning["go"], "Local wording.");
        assert_eq!(pattern.options["max"].default, 9);
        assert_eq!(pattern.options["max"].per_language["go"], 5);
        assert_eq!(pattern.examples.len(), 1);
        assert_eq!(pattern.title, "A");
        assert_eq!(merged.patterns().count(), 1);
    }

    #[test]
    fn extends_cannot_change_other_fields_or_missing_targets() {
        let [a, b] = OVERRIDE_LAYOUT;
        let bad = |text: &str| {
            let map = [a, b, ("l/s/tweak.yaml", text)]
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            Catalog::from_files(map)
        };
        assert!(matches!(
            bad("extends: p/a\nrequirement: A MUST c.\n"),
            Err(Error::Parse { .. })
        ));
        let unknown = tweak("extends: p/none\nseverity: warn\n");
        assert!(
            Catalog::overlay(&base_catalog(), &unknown)
                .unwrap_err()
                .to_string()
                .contains("unknown pattern")
        );
        let wrong_type = tweak("extends: p/a\noptions:\n  max:\n    default: text\n");
        assert!(Catalog::overlay(&base_catalog(), &wrong_type).is_err());
    }

    #[test]
    fn extends_rejects_unknown_option_names() {
        let typo = tweak("extends: p/a\noptions:\n  mx:\n    default: 9\n");
        let err = Catalog::overlay(&base_catalog(), &typo).unwrap_err();
        assert!(err.to_string().contains("unknown option `mx`"), "{err}");
    }

    #[test]
    fn doc_patterns_stay_without_severity_after_overlay() {
        let doc = Catalog::from_files(with(
            "p/s/a.yaml",
            &PATTERN
                .replace("mechanical", "doc")
                .replace("evidence: [x]\n", ""),
        ))
        .unwrap();
        assert!(Catalog::overlay(&doc, &tweak("extends: p/a\nseverity: warn\n")).is_err());
    }

    #[test]
    fn layers_validate_their_own_sources_only() {
        let sources = local(&[
            ("l/pack.yaml", "id: l\ntitle: L\nintro: x\nsections: []\n"),
            (
                "sources.yaml",
                "- ref: d#x-1\n  text: t\n  omitted: project-specific\n",
            ),
        ]);
        let merged = Catalog::overlay(&base_catalog(), &sources).unwrap();
        assert!(merged.patterns().count() == 1);
    }
}

mod write {
    use std::fs;

    use lighthouse_spec::{
        Example, ExampleFile, Expect, Implementation, Kind, OptionSpec, OptionType,
    };

    use super::{fixture::*, *};

    fn rich() -> Pattern {
        let mut pattern = Catalog::from_files(base())
            .unwrap()
            .pattern("p/a")
            .unwrap()
            .clone();
        pattern.id = "p/rich".to_owned();
        pattern.severity_override = Some(Severity::Warn);
        pattern.exceptions = Some("Generated code.".to_owned());
        pattern.citation = Some("Someone 2001".to_owned());
        pattern.tuning.insert("go".into(), "Go wording.".into());
        pattern.options.insert(
            "max".into(),
            OptionSpec {
                kind: OptionType::Int,
                default: json!(3),
                description: "Limit.".into(),
                per_language: [("go".to_owned(), json!(4))].into(),
            },
        );
        pattern.implementation = Some(Implementation::Builtin("p/rich".into()));
        pattern.examples = vec![
            Example {
                name: "bad".into(),
                language: "go".into(),
                kind: Kind::Invalid,
                files: vec![ExampleFile::inline("a.go", "package a\n\nfunc F() {}")],
                expect: vec![Expect {
                    line: 3,
                    message: Some("F".into()),
                }],
                options: Map::from_iter([("max".to_owned(), json!(1))]),
            },
            Example {
                name: "good".into(),
                language: "go".into(),
                kind: Kind::Valid,
                files: vec![ExampleFile::inline("a.go", "package a")],
                expect: Vec::new(),
                options: Map::new(),
            },
        ];
        pattern
    }

    fn write_base(root: &std::path::Path) {
        for (path, text) in base() {
            let target = root.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, text).unwrap();
        }
    }

    #[test]
    fn written_patterns_load_back_unchanged_and_join_the_section_order() {
        let dir = tempfile::tempdir().unwrap();
        write_base(dir.path());
        let pattern = rich();
        Catalog::write_pattern(dir.path(), "s", &pattern).unwrap();
        Catalog::write_pattern(dir.path(), "s", &pattern).unwrap();

        let catalog = Catalog::load(dir.path()).unwrap();
        assert_eq!(catalog.pattern("p/rich"), Some(&pattern));
        let ids: Vec<_> = catalog.patterns().map(|p| &*p.id).collect();
        assert_eq!(ids, ["p/a", "p/rich"]);
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("p/s"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn invalid_patterns_and_unknown_sections_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write_base(dir.path());
        let mut bad = rich();
        bad.intent = String::new();
        assert!(Catalog::write_pattern(dir.path(), "s", &bad).is_err());
        assert!(Catalog::write_pattern(dir.path(), "nope", &rich()).is_err());
        assert!(!dir.path().join("p/s/rich.yaml").exists());
        assert_eq!(Catalog::load(dir.path()).unwrap().patterns().count(), 1);
    }
}
