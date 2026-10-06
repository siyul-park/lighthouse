use lighthouse_plugin::{Analyzer, Ctx, Error, Manifest, Plugin, Registry, Scope};
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
fn register_rejects_bad_plugins_without_side_effects() {
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
