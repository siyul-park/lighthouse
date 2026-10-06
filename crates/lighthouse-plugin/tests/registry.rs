use lighthouse_model::{Capability, Severity};
use lighthouse_plugin::{
    Analyzer, Conventions, Ctx, Error, Indexed, LanguageProvider, Manifest, Plugin, Preset,
    Registry, RuleMeta, Scope, Source, Workspace, options,
};
use serde_json::Value;

struct Node {
    id: &'static str,
    requires: Vec<String>,
}

impl Analyzer for Node {
    fn id(&self) -> &str {
        self.id
    }
    fn requires(&self) -> &[String] {
        &self.requires
    }
    fn scope(&self) -> Scope {
        Scope::Project
    }
    fn run(&self, _: &Ctx) -> Result<Value, Error> {
        Ok(Value::Null)
    }
}

struct Graph {
    plugin: &'static str,
    nodes: &'static [(&'static str, &'static [&'static str])],
}

impl Plugin for Graph {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: self.plugin.to_owned(),
            version: "0".to_owned(),
        }
    }
    fn analyzers(&self) -> Vec<Box<dyn Analyzer>> {
        self.nodes
            .iter()
            .map(|&(id, requires)| {
                Box::new(Node {
                    id,
                    requires: requires.iter().map(|r| (*r).to_owned()).collect(),
                }) as Box<dyn Analyzer>
            })
            .collect()
    }
}

fn graph(nodes: &'static [(&'static str, &'static [&'static str])]) -> Graph {
    Graph { plugin: "t", nodes }
}

fn ids(registry: &Registry, want: &[&str]) -> Result<Vec<String>, Error> {
    Ok(registry
        .order(want.iter().copied())?
        .iter()
        .map(|a| a.id().to_owned())
        .collect())
}

#[test]
fn order_puts_dependencies_first_and_only_what_is_needed() {
    let mut registry = Registry::default();
    registry
        .register(&graph(&[
            ("t/c", &["t/b", "t/a"]),
            ("t/b", &["t/a"]),
            ("t/a", &[]),
            ("t/unused", &[]),
        ]))
        .unwrap();
    assert_eq!(ids(&registry, &["t/c"]).unwrap(), ["t/a", "t/b", "t/c"]);
    assert_eq!(ids(&registry, &["t/b", "t/b"]).unwrap(), ["t/a", "t/b"]);
    registry.validate().unwrap();
}

#[test]
fn cycle_is_reported_with_its_path() {
    let mut registry = Registry::default();
    registry
        .register(&graph(&[
            ("t/a", &["t/b"]),
            ("t/b", &["t/c"]),
            ("t/c", &["t/a"]),
        ]))
        .unwrap();
    let err = registry.validate().unwrap_err();
    assert_eq!(err.to_string(), "analyzer cycle: t/a -> t/b -> t/c -> t/a");
}

#[test]
fn missing_requirement_is_reported() {
    let mut registry = Registry::default();
    registry.register(&graph(&[("t/a", &["t/nope"])])).unwrap();
    let err = registry.validate().unwrap_err();
    assert!(
        matches!(err, Error::MissingAnalyzer { id, required_by } if id == "t/nope" && required_by == "t/a")
    );
}

#[test]
fn registry_register() {
    let mut registry = Registry::default();
    let wrong_prefix = graph(&[("other/a", &[])]);
    assert!(matches!(
        registry.register(&wrong_prefix),
        Err(Error::Prefix { .. })
    ));
    let twice = graph(&[("t/a", &[]), ("t/a", &[])]);
    assert!(matches!(
        registry.register(&twice),
        Err(Error::Duplicate(_))
    ));
    assert!(!registry.has_plugin("t"));

    registry.register(&graph(&[("t/a", &[])])).unwrap();
    assert!(matches!(
        registry.register(&graph(&[("t/b", &[])])),
        Err(Error::Duplicate(_))
    ));
}

#[test]
fn plugins_are_listed_in_registration_order() {
    let mut registry = Registry::default();
    registry
        .register(&Graph {
            plugin: "b",
            nodes: &[],
        })
        .unwrap();
    registry
        .register(&Graph {
            plugin: "a",
            nodes: &[],
        })
        .unwrap();
    assert_eq!(registry.plugins().collect::<Vec<_>>(), ["b", "a"]);
}

struct Lang {
    id: &'static str,
    fallback: bool,
    priority: i32,
}

impl LanguageProvider for Lang {
    fn id(&self) -> &str {
        self.id
    }
    fn globs(&self) -> &[String] {
        &[]
    }
    fn conventions(&self) -> Conventions {
        Conventions::default()
    }
    fn capabilities(&self) -> &[Capability] {
        &[]
    }
    fn fallback(&self) -> bool {
        self.fallback
    }
    fn priority(&self) -> i32 {
        self.priority
    }
    fn index(&self, _: &Workspace, _: &[Source]) -> Result<Indexed, Error> {
        Ok(Indexed::default())
    }
}

struct Langs(&'static str, &'static [(&'static str, bool, i32)]);

impl Plugin for Langs {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: self.0.to_owned(),
            version: "0".to_owned(),
        }
    }
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        self.1
            .iter()
            .map(|&(id, fallback, priority)| {
                Box::new(Lang {
                    id,
                    fallback,
                    priority,
                }) as Box<dyn LanguageProvider>
            })
            .collect()
    }
}

#[test]
fn fallback_providers_come_after_regular_ones_whatever_the_registration_order() {
    let mut registry = Registry::default();
    registry
        .register(&Langs("a", &[("text", true, 0)]))
        .unwrap();
    registry
        .register(&Langs("b", &[("go", false, 0), ("py", false, 0)]))
        .unwrap();
    let ids: Vec<_> = registry
        .languages()
        .map(|(_, l)| l.id().to_owned())
        .collect();
    assert_eq!(ids, ["go", "py", "text"]);
}

#[test]
fn higher_priority_providers_come_first_and_ties_keep_registration_order() {
    let mut registry = Registry::default();
    registry
        .register(&Langs("a", &[("low", false, 0), ("tie", false, 5)]))
        .unwrap();
    registry
        .register(&Langs("b", &[("high", false, 9), ("tie2", false, 5)]))
        .unwrap();
    let ids: Vec<_> = registry
        .languages()
        .map(|(_, l)| l.id().to_owned())
        .collect();
    assert_eq!(ids, ["high", "tie", "tie2", "low"]);
}

fn meta(id: &str, severity: Severity, strict: bool) -> RuleMeta {
    RuleMeta {
        id: id.to_owned(),
        severity,
        scope: Scope::File,
        description: String::new(),
        docs: String::new(),
        analyzers: Vec::new(),
        capabilities: Vec::new(),
        citation: None,
        strict,
    }
}

#[test]
fn preset_standard_puts_strict_rules_only_in_the_strict_preset() {
    let metas = [
        meta("p/a", Severity::Warn, false),
        meta("p/b", Severity::Error, true),
    ];

    let presets = Preset::standard("p", &metas);

    let ids: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["p/recommended", "p/strict"]);
    assert_eq!(presets[0].rules.len(), 1);
    assert_eq!(presets[0].rules["p/a"].level, Some(Severity::Warn));
    assert_eq!(presets[1].rules.len(), 2);
    assert_eq!(Preset::standard("p", &metas[..1]).len(), 1);
}

#[test]
fn options_deserializes_rule_options_and_names_the_rule_on_failure() {
    #[derive(serde::Deserialize, Debug)]
    #[serde(deny_unknown_fields)]
    struct Max {
        max: u64,
    }
    let ok: Max = options("p/a", &serde_json::from_str(r#"{"max": 3}"#).unwrap()).unwrap();
    assert_eq!(ok.max, 3);

    let err = options::<Max>("p/a", &serde_json::from_str(r#"{"nope": 1}"#).unwrap()).unwrap_err();
    assert!(matches!(&err, Error::Options { rule, .. } if rule == "p/a"));
}
