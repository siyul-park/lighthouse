use std::collections::BTreeMap;

use lighthouse_model::{File, Fragment, Options, Project, Severity};
use lighthouse_plugin::{Ctx, Error, Facts, Scope, Workspace};
use serde_json::json;

fn options(max: u64) -> Options {
    serde_json::from_value(json!({ "max": max })).unwrap()
}

fn file() -> File {
    File {
        path: "dir/a.txt".into(),
        lang: "text".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
    }
}

fn run_rule(text: &str, options: &Options) -> Result<Vec<lighthouse_model::Diagnostic>, Error> {
    let registry = lighthouse_builtin::registry();
    let file = file();
    let ws = Workspace { root: ".".into() };
    let project = Project::merge([Fragment::default()]);
    let mut facts = Facts::new();
    let ctx = |facts: &Facts| -> Result<Vec<_>, Error> {
        let ctx = Ctx {
            ws: &ws,
            project: &project,
            file: Some((&file, text)),
            facts,
        };
        registry
            .rule("core/max-file-lines")
            .unwrap()
            .check(&ctx, options)
    };
    let analyzers = registry.order(["core/line-count"])?;
    for analyzer in analyzers {
        let value = analyzer.run(&Ctx {
            ws: &ws,
            project: &project,
            file: Some((&file, text)),
            facts: &facts,
        })?;
        facts.insert((analyzer.id().to_owned(), "dir/a.txt".to_owned()), value);
    }
    ctx(&facts)
}

#[test]
fn registry_is_valid_and_exposes_core() {
    let registry = lighthouse_builtin::registry();
    registry.validate().unwrap();
    assert!(registry.has_plugin("core"));
    let meta = registry.rule("core/max-file-lines").unwrap().meta();
    assert_eq!(meta.scope, Scope::File);
    assert_eq!(meta.analyzers, ["core/line-count"]);
}

#[test]
fn recommended_preset_derives_levels_from_rule_meta() {
    let registry = lighthouse_builtin::registry();
    let preset = registry.preset("core/recommended").unwrap();
    let want: BTreeMap<_, _> = registry
        .rules()
        .map(|r| (r.meta().id.clone(), Some(r.meta().severity)))
        .collect();
    let got: BTreeMap<_, _> = preset
        .rules
        .iter()
        .map(|(id, c)| (id.clone(), c.level))
        .collect();
    assert_eq!(got, want);
    assert_eq!(want["core/max-file-lines"], Some(Severity::Warn));
}

#[test]
fn rule_reports_lines_beyond_the_limit() {
    let found = run_rule("a\nb\nc\n", &options(2)).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].span.start.line, 3);
    assert_eq!(found[0].evidence, json!({ "lines": 3, "max": 2 }));
    assert!(run_rule("a\nb\n", &options(2)).unwrap().is_empty());
}

#[test]
fn rule_validates_options() {
    let registry = lighthouse_builtin::registry();
    let rule = registry.rule("core/max-file-lines").unwrap();
    rule.validate(&Options::new()).unwrap();
    rule.validate(&options(5)).unwrap();
    let bad: Options = serde_json::from_value(json!({ "mx": 5 })).unwrap();
    assert!(matches!(rule.validate(&bad), Err(Error::Options { .. })));
}

#[test]
fn missing_fact_is_a_distinct_error() {
    let registry = lighthouse_builtin::registry();
    let file = file();
    let ws = Workspace { root: ".".into() };
    let project = Project::default();
    let facts = Facts::new();
    let ctx = Ctx {
        ws: &ws,
        project: &project,
        file: Some((&file, "")),
        facts: &facts,
    };
    let err = registry
        .rule("core/max-file-lines")
        .unwrap()
        .check(&ctx, &Options::new());
    assert!(matches!(err, Err(Error::MissingFact(_))));
}

#[test]
fn registered_rules_are_exactly_the_catalog_patterns_implemented_by_builtin() {
    let registry = lighthouse_builtin::registry();
    let catalog = lighthouse_spec::Catalog::bundled();
    let registered: std::collections::BTreeSet<_> =
        registry.rules().map(|r| r.meta().id.clone()).collect();
    for id in &registered {
        let pattern = catalog
            .pattern(id)
            .unwrap_or_else(|| panic!("{id} has no pattern"));
        assert_ne!(
            pattern.enforcement,
            lighthouse_spec::Enforcement::Doc,
            "{id}"
        );
        assert_eq!(
            pattern.implementation,
            Some(lighthouse_spec::Implementation::Builtin(id.clone())),
            "{id}"
        );
    }
    let implemented: std::collections::BTreeSet<_> = catalog
        .patterns()
        .filter(|p| {
            matches!(
                p.implementation,
                Some(lighthouse_spec::Implementation::Builtin(_))
            )
        })
        .map(|p| p.id.clone())
        .collect();
    assert_eq!(registered, implemented);
}

#[test]
fn catalog_examples_drive_the_rule() {
    let catalog = lighthouse_spec::Catalog::bundled();
    let pattern = catalog.pattern("core/max-file-lines").unwrap();
    for example in &pattern.examples {
        let options: Options = example.options.clone();
        let text = example.files[0].text();
        let found = run_rule(text, &options).unwrap();
        match example.kind {
            lighthouse_spec::Kind::Valid => assert!(found.is_empty(), "{}", example.name),
            lighthouse_spec::Kind::Invalid => {
                let lines: Vec<_> = found.iter().map(|d| d.span.start.line).collect();
                let want: Vec<_> = example.expect.iter().map(|e| e.line).collect();
                assert_eq!(lines, want, "{}", example.name);
            }
        }
    }
}

#[test]
fn default_limit_comes_from_the_catalog() {
    let text = "x\n".repeat(1001);
    assert_eq!(run_rule(&text, &Options::new()).unwrap().len(), 1);
    assert!(
        run_rule(&"x\n".repeat(1000), &Options::new())
            .unwrap()
            .is_empty()
    );
}
