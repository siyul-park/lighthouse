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
