//! Findings as queries: the configuration of a rule, what it found, and the
//! runtime a graph runs it with.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use lighthouse_graph::{Graph, Options as GraphOptions};
use lighthouse_model::{
    Diagnostic, Fingerprint, Options, Position, RunScope, Severity, Span, ucm::Applicability,
};
use lighthouse_plugin::{
    Analyzer, AnalyzerContext, AnalyzerManifest, Annotations, Context, Error, FINDINGS, Findings,
    Judged, Judging, NoModels, Plugin, PluginManifest, Registry, Rule, RuleManifest, Runtime,
    Workspace, judge, set_judged,
};
use serde_json::Value;

const RULE: &str = "t/found";

struct Found(RuleManifest);

impl Found {
    fn new() -> Self {
        Self(RuleManifest {
            id: RULE.to_owned(),
            uid: None,
            severity: Severity::Warn,
            scope: RunScope::Project,
            description: String::new(),
            docs: String::new(),
            analyzers: Vec::new(),
            capabilities: Vec::new(),
            applicability: Applicability::default(),
            revision: Some("1".to_owned()),
            tools: Vec::new(),
        })
    }
}

impl Rule for Found {
    fn manifest(&self) -> &RuleManifest {
        &self.0
    }
    fn validate(&self, _: &Options) -> Result<(), Error> {
        Ok(())
    }
    fn check(&self, _: &Context, _: &Options) -> Result<Vec<Diagnostic>, Error> {
        Ok(vec![Diagnostic::new(
            RULE,
            Severity::Warn,
            "found".to_owned(),
            Path::new("a.txt"),
            Span {
                start: Position { line: 1, col: 1 },
                end: Position { line: 1, col: 1 },
            },
            Fingerprint::new(RULE, "a.txt", ""),
        )])
    }
}

struct Counting(AnalyzerManifest);

impl Analyzer for Counting {
    fn manifest(&self) -> &AnalyzerManifest {
        &self.0
    }
    fn run(&self, _: &AnalyzerContext) -> Result<Value, Error> {
        Ok(Value::Null)
    }
}

struct Bundle(PluginManifest);

impl Plugin for Bundle {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }
    fn analyzers(&self) -> Vec<Box<dyn Analyzer>> {
        vec![Box::new(Counting(AnalyzerManifest {
            id: "t/count".to_owned(),
            requires: Vec::new(),
            scope: RunScope::Project,
        }))]
    }
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![Box::new(Found::new())]
    }
}

fn registry() -> Arc<Registry> {
    let mut registry = Registry::default();
    registry
        .register(&Bundle(PluginManifest {
            id: "t".to_owned(),
            version: "0".to_owned(),
        }))
        .unwrap();
    Arc::new(registry)
}

fn nothing(_: &Graph, _: &str) -> Result<Arc<Option<String>>, lighthouse_graph::Error> {
    Ok(Arc::new(None))
}

fn not_overlaid(_: &Graph, _: &str) -> Result<Option<String>, lighthouse_graph::Error> {
    Ok(None)
}

fn graph() -> Graph {
    let runtime = Runtime::new(
        registry(),
        (nothing, not_overlaid),
        (Workspace::new(""), false),
    );
    Graph::in_memory(GraphOptions::default().data(Arc::new(runtime))).unwrap()
}

fn judged(options: &str) -> Judged {
    let options: Options = serde_json::from_str(options).unwrap();
    Judged::of(&options, Applicability::default(), "env".to_owned())
}

#[test]
fn judged_of() {
    let found = judged(r#"{"max":3}"#);

    assert_eq!(found.options, r#"{"max":3}"#);
    assert_eq!(found.environment, "env");
}

#[test]
fn judged_variant() {
    let a = judged(r#"{"max":3}"#);
    let b = Judged {
        environment: "other".to_owned(),
        ..a.clone()
    };

    assert_eq!(a.variant(), b.variant());
    assert_ne!(a.variant(), judged(r#"{"max":4}"#).variant());
}

#[test]
fn findings_diagnostics() {
    let none = Findings::default();
    assert!(none.diagnostics().is_err());

    let some = Findings {
        found: "[]".to_owned(),
        notices: Vec::new(),
    };
    assert!(some.diagnostics().unwrap().is_empty());
}

#[test]
fn annotations_of() {
    let annotations = Annotations::default();
    assert!(annotations.of(Path::new("a")).is_none());

    annotations.insert(PathBuf::from("a"), Vec::new());
    assert_eq!(annotations.of(Path::new("a")), Some(Vec::new()));
}

#[test]
fn registry_analyzer() {
    let registry = registry();

    assert!(registry.analyzer("t/count").is_some());
    assert!(registry.analyzer("t/none").is_none());
}

#[test]
fn runtime_of() {
    assert!(Runtime::of(&graph()).is_ok());

    let bare = Graph::in_memory(GraphOptions::default()).unwrap();
    assert!(Runtime::of(&bare).is_err());
}

#[test]
fn runtime_take_times() {
    let graph = graph();
    let runtime = Runtime::of(&graph).unwrap();

    assert!(runtime.take_times().is_empty());
}

#[test]
fn set_judged_names_the_variant() {
    let mut graph = graph();
    let found = judged("{}");

    let variant = set_judged(&mut graph, RULE, found.clone()).unwrap();

    assert_eq!(variant, found.variant());
}

#[test]
fn judge_runs_the_rule() {
    let mut graph = graph();
    let found = judged("{}");
    set_judged(&mut graph, RULE, found.clone()).unwrap();
    let annotations = Annotations::default();

    let said = judge(
        &graph,
        RULE,
        "",
        &found,
        &Judging {
            models: &NoModels,
            annotations: &annotations,
            workspace: None,
            documents: &[],
        },
    )
    .unwrap();

    assert_eq!(said.diagnostics().unwrap().len(), 1);
    let node = graph
        .query(
            &FINDINGS,
            &(RULE.to_owned(), String::new(), found.variant()),
        )
        .unwrap();
    assert_eq!(node.found, said.found);
}
