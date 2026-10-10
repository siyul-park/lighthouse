use lighthouse_model::RunScope;
use lighthouse_plugin::{
    Analyzer, AnalyzerManifest, Ctx, Error, Indexed, LanguageProvider, Plugin, PluginManifest,
    ProviderManifest, Registry, Source, Workspace, options,
};
use serde_json::Value;

struct Node(AnalyzerManifest);

impl Analyzer for Node {
    fn manifest(&self) -> &AnalyzerManifest {
        &self.0
    }
    fn run(&self, _: &Ctx) -> Result<Value, Error> {
        Ok(Value::Null)
    }
}

struct Graph {
    manifest: PluginManifest,
    nodes: &'static [(&'static str, &'static [&'static str])],
}

impl Graph {
    fn new(plugin: &str, nodes: &'static [(&'static str, &'static [&'static str])]) -> Self {
        Self {
            manifest: PluginManifest {
                id: plugin.to_owned(),
                version: "0".to_owned(),
            },
            nodes,
        }
    }
}

impl Plugin for Graph {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    fn analyzers(&self) -> Vec<Box<dyn Analyzer>> {
        self.nodes
            .iter()
            .map(|&(id, requires)| {
                Box::new(Node(AnalyzerManifest {
                    id: id.to_owned(),
                    requires: requires.iter().map(|r| (*r).to_owned()).collect(),
                    scope: RunScope::Project,
                })) as Box<dyn Analyzer>
            })
            .collect()
    }
}

fn graph(nodes: &'static [(&'static str, &'static [&'static str])]) -> Graph {
    Graph::new("t", nodes)
}

fn ids(registry: &Registry, want: &[&str]) -> Result<Vec<String>, Error> {
    Ok(registry
        .order(want.iter().copied())?
        .iter()
        .map(|a| a.manifest().id.clone())
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
    registry.register(&Graph::new("b", &[])).unwrap();
    registry.register(&Graph::new("a", &[])).unwrap();
    assert_eq!(registry.plugins().collect::<Vec<_>>(), ["b", "a"]);
}

struct Lang(ProviderManifest);

impl LanguageProvider for Lang {
    fn manifest(&self) -> &ProviderManifest {
        &self.0
    }
    fn index(&self, _: &Workspace, _: &[Source]) -> Result<Indexed, Error> {
        Ok(Indexed::default())
    }
}

struct Langs(PluginManifest, &'static [(&'static str, bool, i32)]);

impl Langs {
    fn new(plugin: &str, languages: &'static [(&'static str, bool, i32)]) -> Self {
        let manifest = PluginManifest {
            id: plugin.to_owned(),
            version: "0".to_owned(),
        };
        Self(manifest, languages)
    }
}

impl Plugin for Langs {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        self.1
            .iter()
            .map(|&(id, fallback, priority)| {
                Box::new(Lang(ProviderManifest {
                    fallback,
                    priority,
                    ..ProviderManifest::new(id, Vec::new())
                })) as Box<dyn LanguageProvider>
            })
            .collect()
    }
}

#[test]
fn fallback_providers_come_after_regular_ones_whatever_the_registration_order() {
    let mut registry = Registry::default();
    registry
        .register(&Langs::new("a", &[("text", true, 0)]))
        .unwrap();
    registry
        .register(&Langs::new("b", &[("go", false, 0), ("py", false, 0)]))
        .unwrap();
    let ids: Vec<_> = registry
        .languages()
        .map(|(_, l)| l.manifest().id.clone())
        .collect();
    assert_eq!(ids, ["go", "py", "text"]);
}

#[test]
fn higher_priority_providers_come_first_and_ties_keep_registration_order() {
    let mut registry = Registry::default();
    registry
        .register(&Langs::new("a", &[("low", false, 0), ("tie", false, 5)]))
        .unwrap();
    registry
        .register(&Langs::new("b", &[("high", false, 9), ("tie2", false, 5)]))
        .unwrap();
    let ids: Vec<_> = registry
        .languages()
        .map(|(_, l)| l.manifest().id.clone())
        .collect();
    assert_eq!(ids, ["high", "tie", "tie2", "low"]);
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
