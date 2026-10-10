use std::{collections::BTreeMap, path::Path};

use lighthouse_model::RunScope;
use lighthouse_model::{File, Fragment, Options, Project, Severity};
use lighthouse_plugin::{Ctx, Error, Facts, Workspace};
use lighthouse_spec::{Catalog, Config, Rules};
use serde_json::json;

fn options(max: u64) -> Options {
    serde_json::from_value(json!({ "max": max })).unwrap()
}

/// The rules of the standard project `name`, as a configuration that extends
/// it resolves them.
fn preset(name: &str) -> Rules {
    Config::parse_inline(&format!("extends = [\"{name}\"]"))
        .unwrap()
        .resolve(Path::new(""), "", &Catalog::bundled().projects().unwrap())
        .unwrap()
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
    let registry = lighthouse_checks::registry();
    let file = file();
    let ws = Workspace::new(".");
    let project = Project::merge([Fragment::default()]);
    let facts = Facts::new();
    let ctx = Ctx {
        ws: &ws,
        project: &project,
        file: Some((&file, text)),
        facts: &facts,
        keys: &lighthouse_plugin::NoKeys,
        trusted: false,
        memo: &lighthouse_plugin::Memo::default(),
        applies: lighthouse_model::Applicability::default(),
    };
    registry
        .rule("core/max-lines")
        .unwrap()
        .check(&ctx, options)
}

#[test]
fn registry_is_valid_and_exposes_core() {
    let registry = lighthouse_checks::registry();
    registry.validate().unwrap();
    assert!(registry.has_plugin("core"));
    let meta = registry.rule("core/max-lines").unwrap().manifest();
    assert_eq!(meta.scope, RunScope::File);
    assert!(
        meta.analyzers.is_empty(),
        "the lines of a file are read from its text"
    );
}

#[test]
fn a_rule_exists_for_a_decision_a_program_checks() {
    let registry = lighthouse_checks::registry();
    let meta = registry.rule("core/max-lines").unwrap().manifest();
    assert_eq!(meta.id, "core/max-lines");
    assert_eq!(meta.severity, Severity::Warn);
    assert_eq!(meta.scope, RunScope::File);
    for judged in ["design/no-private-types", "design/advisory-signals"] {
        assert!(registry.rule(judged).is_none(), "{judged}");
    }
    for annotation in ["core/allow-reason", "core/no-unused-allow"] {
        assert!(registry.rule(annotation).is_some(), "{annotation}");
    }
}

#[test]
fn recommended_preset_derives_levels_from_rule_meta() {
    let registry = lighthouse_checks::registry();
    let preset = preset("core/recommended");
    let want: BTreeMap<_, _> = registry
        .rules()
        .filter(|r| lighthouse_plugin::plugin_of(&r.manifest().id) == "core")
        .map(|r| (r.manifest().id.clone(), Some(r.manifest().severity)))
        .collect();
    let got: BTreeMap<_, _> = preset.iter().map(|(id, c)| (id.clone(), c.level)).collect();
    assert_eq!(got, want);
    assert_eq!(want["core/max-lines"], Some(Severity::Warn));
}

#[test]
fn the_standard_projects_hold_exactly_the_rules_the_registry_has() {
    let registry = lighthouse_checks::registry();
    let catalog = Catalog::bundled();
    let projects = catalog.projects().unwrap();
    for pack in &catalog.packs {
        let ours: Vec<String> = registry
            .rules()
            .filter(|r| lighthouse_plugin::plugin_of(&r.manifest().id) == pack.id)
            .map(|r| r.manifest().id.clone())
            .collect();
        let strict = format!("{}/strict", pack.id);
        let widest = if projects.get(&strict).is_some() {
            strict
        } else {
            format!("{}/recommended", pack.id)
        };
        let held: Vec<String> = preset(&widest).keys().cloned().collect();
        assert_eq!(held, ours, "{}", pack.id);
    }
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
    let registry = lighthouse_checks::registry();
    let rule = registry.rule("core/max-lines").unwrap();
    rule.validate(&Options::new()).unwrap();
    rule.validate(&options(5)).unwrap();
    let bad: Options = serde_json::from_value(json!({ "mx": 5 })).unwrap();
    assert!(matches!(rule.validate(&bad), Err(Error::Options { .. })));
}

#[test]
fn registered_rules_are_exactly_the_automated_catalog_decisions() {
    use lighthouse_spec::CheckKind;

    let registry = lighthouse_checks::registry();
    let catalog = lighthouse_spec::Catalog::bundled();
    let registered: std::collections::BTreeSet<_> =
        registry.rules().map(|r| r.manifest().id.clone()).collect();
    for id in &registered {
        let decision = catalog
            .decision(id)
            .unwrap_or_else(|| panic!("{id} has no decision"));
        match decision.check.as_ref().map(|c| &c.kind) {
            Some(CheckKind::Builtin(rule)) => {
                assert!(rule.named().is_none_or(|rule| rule == id), "{id}");
            }
            Some(CheckKind::Cel(_)) => {}
            other => panic!("{id} is registered but its decision's check is {other:?}"),
        }
    }
    let checked: std::collections::BTreeSet<_> = catalog
        .decisions()
        .filter(|d| d.automated())
        .map(|d| d.id().to_owned())
        .collect();
    assert_eq!(registered, checked);
}

#[test]
fn every_fix_names_a_registered_fixer_and_the_registry_holds_no_other() {
    use lighthouse_model::Capability;
    use lighthouse_spec::{FixKind, OpSpec};

    let registry = lighthouse_checks::registry();
    let catalog = lighthouse_spec::Catalog::bundled();
    let mut fixable = std::collections::BTreeSet::new();
    for decision in catalog.decisions() {
        let Some(fix) = &decision.fix else { continue };
        let id = &decision.id().to_owned();
        fixable.insert(id.clone());
        assert!(registry.rule(id).is_some(), "{id} has a fix but no rule");
        let fixer = registry
            .fixer(id)
            .unwrap_or_else(|| panic!("{id} has a fix but no registered fixer"));
        assert_eq!(&fixer.manifest().id, id);
        assert_eq!(fixer.manifest().requires, fix.requires, "{id}");
        let FixKind::Ops { ops } = &fix.kind else {
            continue;
        };
        for op in ops {
            match op {
                OpSpec::Reorder { by, .. } => {
                    for key in by {
                        assert!(
                            registry.order_keys().any(|k| k.manifest().id == *key),
                            "{id} orders by unregistered key {key}"
                        );
                    }
                    assert!(fix.requires.contains(&Capability::Extent), "{id}");
                }
                OpSpec::Move { .. } => {
                    assert!(fix.requires.contains(&Capability::Extent), "{id}");
                }
                OpSpec::Rename { .. } => {
                    assert!(fix.requires.contains(&Capability::ReferenceSites), "{id}");
                }
                OpSpec::Delete { node: Some(_), .. } => {
                    assert!(fix.requires.contains(&Capability::Extent), "{id}");
                }
                OpSpec::Delete { .. } | OpSpec::Replace { .. } => {}
            }
        }
    }
    let registered: std::collections::BTreeSet<_> =
        registry.fixers().map(|f| f.manifest().id.clone()).collect();
    assert_eq!(registered, fixable);
    assert!(!fixable.is_empty());
}

#[test]
fn the_bundled_fixes_say_what_each_rule_needs_to_be_fixed() {
    let catalog = lighthouse_spec::Catalog::bundled();
    let safety = |id: &str| {
        catalog
            .decision(id)
            .and_then(|d| d.fix.as_ref())
            .map(|f| f.safety.to_string())
    };
    for id in ["design/declaration-groups", "testing/file-layout"] {
        assert_eq!(safety(id).as_deref(), Some("safe"), "{id}");
    }
    for id in [
        "design/contiguity",
        "design/callers-before-callees",
        "design/no-banners",
        "core/no-unused-allow",
    ] {
        assert_eq!(safety(id).as_deref(), Some("suggested"), "{id}");
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

#[test]
fn bundled_plugins_provide_only_the_fallback_text_language() {
    let registry = lighthouse_checks::registry();
    let plugins: Vec<_> = registry.plugins().collect();
    assert_eq!(plugins, ["core", "metrics", "design", "testing"]);
    let languages: Vec<_> = registry
        .languages()
        .map(|(_, l)| l.manifest().id.clone())
        .collect();
    assert_eq!(languages, ["text"]);
    let design: Vec<_> = registry
        .rules()
        .filter(|r| lighthouse_plugin::plugin_of(&r.manifest().id) == "design")
        .map(|r| r.manifest().id.clone())
        .collect();
    assert_eq!(
        design,
        [
            "design/callers-before-callees",
            "design/complexity",
            "design/contiguity",
            "design/coupling",
            "design/declaration-groups",
            "design/exported-doc",
            "design/layers",
            "design/max-name-words",
            "design/no-banners",
            "design/no-mutable-globals",
            "design/no-redundant-qualifiers",
            "design/no-single-use-wrapper",
            "design/prefer-method",
            "design/private-helper-callers",
            "design/tiny-modules",
            "design/unique-type-names"
        ]
    );
    let recommended = preset("design/recommended");
    assert_eq!(recommended.len(), design.len() - 4);
    for strict in [
        "design/private-helper-callers",
        "design/max-name-words",
        "design/unique-type-names",
        "design/tiny-modules",
    ] {
        assert!(!recommended.contains_key(strict), "{strict}");
    }
    assert!(recommended.contains_key("design/layers"));
    assert_eq!(preset("design/strict").len(), design.len());
    assert_eq!(preset("testing/recommended").len(), 5);
}
