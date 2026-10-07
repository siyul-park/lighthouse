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
    let ws = Workspace::new(".");
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
        facts.insert(
            (analyzer.manifest().id.clone(), "dir/a.txt".to_owned()),
            value,
        );
    }
    ctx(&facts)
}

#[test]
fn registry_is_valid_and_exposes_core() {
    let registry = lighthouse_builtin::registry();
    registry.validate().unwrap();
    assert!(registry.has_plugin("core"));
    let meta = registry.rule("core/max-file-lines").unwrap().manifest();
    assert_eq!(meta.scope, Scope::File);
    assert_eq!(meta.analyzers, ["core/line-count"]);
}

#[test]
fn recommended_preset_derives_levels_from_rule_meta() {
    let registry = lighthouse_builtin::registry();
    let preset = registry.preset("core/recommended").unwrap();
    let want: BTreeMap<_, _> = registry
        .rules()
        .filter(|r| lighthouse_plugin::plugin_of(&r.manifest().id) == "core")
        .map(|r| (r.manifest().id.clone(), Some(r.manifest().severity)))
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
    let ws = Workspace::new(".");
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
fn registered_rules_are_exactly_the_implemented_catalog_patterns() {
    let registry = lighthouse_builtin::registry();
    let catalog = lighthouse_spec::Catalog::bundled();
    let registered: std::collections::BTreeSet<_> =
        registry.rules().map(|r| r.manifest().id.clone()).collect();
    for id in &registered {
        let pattern = catalog
            .pattern(id)
            .unwrap_or_else(|| panic!("{id} has no pattern"));
        assert_ne!(
            pattern.enforcement,
            lighthouse_spec::Enforcement::Doc,
            "{id}"
        );
        match &pattern.implementation {
            Some(lighthouse_spec::Implementation::Builtin(rule)) => assert_eq!(rule, id),
            Some(lighthouse_spec::Implementation::Declarative(_)) => {}
            None => panic!("{id} is registered but its pattern has no implementation"),
        }
    }
    let implemented: std::collections::BTreeSet<_> = catalog
        .patterns()
        .filter(|p| {
            matches!(
                p.implementation,
                Some(
                    lighthouse_spec::Implementation::Builtin(_)
                        | lighthouse_spec::Implementation::Declarative(_)
                )
            )
        })
        .map(|p| p.id.clone())
        .collect();
    assert_eq!(registered, implemented);
}

#[test]
fn every_fix_names_a_registered_fixer_and_the_registry_holds_no_other() {
    use lighthouse_model::Capability;
    use lighthouse_spec::{FixKind, OpSpec};

    let registry = lighthouse_builtin::registry();
    let catalog = lighthouse_spec::Catalog::bundled();
    let mut fixable = std::collections::BTreeSet::new();
    for pattern in catalog.patterns() {
        let Some(fix) = &pattern.fix else { continue };
        let id = &pattern.id;
        fixable.insert(id.clone());
        assert!(registry.rule(id).is_some(), "{id} has a fix but no rule");
        let fixer = registry
            .fixer(id)
            .unwrap_or_else(|| panic!("{id} has a fix but no registered fixer"));
        assert_eq!(&fixer.manifest().id, id);
        assert_eq!(fixer.manifest().requires, fix.requires, "{id}");
        let FixKind::Ops(ops) = &fix.kind else {
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
            .pattern(id)
            .and_then(|p| p.fix.as_ref())
            .map(|f| f.safety.to_string())
    };
    for id in ["design/declaration-groups", "testing/test-file-layout"] {
        assert_eq!(safety(id).as_deref(), Some("safe"), "{id}");
    }
    for id in [
        "design/related-symbols-close",
        "design/callers-before-callees",
        "design/section-banners",
        "core/unused-allow",
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
    let registry = lighthouse_builtin::registry();
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
            "design/complexity-signal",
            "design/coupling-signal",
            "design/declaration-groups",
            "design/exported-doc",
            "design/no-exported-mutable-global",
            "design/no-redundant-qualifiers",
            "design/private-helper-callers",
            "design/receiver-owned-behavior",
            "design/related-symbols-close",
            "design/section-banners",
            "design/single-use-wrapper"
        ]
    );
    let preset = registry.preset("design/recommended").unwrap();
    assert_eq!(preset.rules.len(), design.len() - 1);
    assert!(!preset.rules.contains_key("design/private-helper-callers"));
    let strict = registry.preset("design/strict").unwrap();
    assert_eq!(strict.rules.len(), design.len());
    let testing = registry.preset("testing/recommended").unwrap();
    assert_eq!(testing.rules.len(), 5);
}
