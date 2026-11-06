//! Findings as queries: the configuration of a rule, what it found, and the
//! runtime a graph runs it with.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use lighthouse_graph::{Graph, Options as GraphOptions};
use lighthouse_model::{
    Diagnostic, Fingerprint, Options, Position, RunScope, Severity, Span, ucm::Applicability,
};
use lighthouse_plugin::{
    Analyzer, AnalyzerContext, AnalyzerManifest, Annotations, COMMAND, Context, Error, FINDINGS,
    Findings, Judged, Judging, NoModels, Plugin, PluginManifest, Registry, Rule, RuleManifest,
    Runtime, Workspace, judge, quiet_report, run_batches, set_judged, set_run, state_report,
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

/// Counts the runs of its program over a batch.
struct Batching(RuleManifest, AtomicUsize);

impl Rule for Batching {
    fn manifest(&self) -> &RuleManifest {
        &self.0
    }
    fn validate(&self, _: &Options) -> Result<(), Error> {
        Ok(())
    }
    fn check(&self, _: &Context, _: &Options) -> Result<Vec<Diagnostic>, Error> {
        Ok(Vec::new())
    }
    fn batches(&self) -> bool {
        true
    }
    fn batch(
        &self,
        _: &Context,
        _: &Options,
        _: &[PathBuf],
    ) -> Result<BTreeMap<PathBuf, Vec<Diagnostic>>, Error> {
        self.1.fetch_add(1, Ordering::SeqCst);
        Ok(BTreeMap::new())
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

#[test]
fn run_batches_runs_one_program_over_the_stale_batches_of_a_rule() {
    let rule = Arc::new(Batching(Found::new().0, AtomicUsize::new(0)));
    let mut registry = Registry::default();
    registry
        .register(&Shared(
            PluginManifest {
                id: "t".to_owned(),
                version: "0".to_owned(),
            },
            rule.clone(),
        ))
        .unwrap();
    let runtime = Runtime::new(
        Arc::new(registry),
        (nothing, not_overlaid),
        (Workspace::new(""), false),
    );
    let mut graph = Graph::in_memory(GraphOptions::default().data(Arc::new(runtime))).unwrap();
    let found = judged("{}");
    let variant = set_judged(&mut graph, RULE, found).unwrap();
    let wanted = vec![
        (RULE.to_owned(), variant.clone(), vec!["a".to_owned()]),
        (RULE.to_owned(), variant, vec!["b".to_owned()]),
    ];

    run_batches(&graph, wanted.clone()).unwrap();

    assert_eq!(rule.1.load(Ordering::SeqCst), 1);
    for key in &wanted {
        assert!(graph.query(&COMMAND, key).unwrap().answered.is_empty());
    }
    assert_eq!(
        rule.1.load(Ordering::SeqCst),
        1,
        "the nodes took the answer"
    );
}

/// The rule the test counts the runs of, given to a registry.
struct Shared(PluginManifest, Arc<Batching>);

struct Delegate(Arc<Batching>);

impl Rule for Delegate {
    fn manifest(&self) -> &RuleManifest {
        self.0.manifest()
    }
    fn validate(&self, options: &Options) -> Result<(), Error> {
        self.0.validate(options)
    }
    fn check(&self, ctx: &Context, options: &Options) -> Result<Vec<Diagnostic>, Error> {
        self.0.check(ctx, options)
    }
    fn batches(&self) -> bool {
        true
    }
    fn batch(
        &self,
        ctx: &Context,
        options: &Options,
        files: &[PathBuf],
    ) -> Result<BTreeMap<PathBuf, Vec<Diagnostic>>, Error> {
        self.0.batch(ctx, options, files)
    }
}

impl Plugin for Shared {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![Box::new(Delegate(self.1.clone()))]
    }
}

/// A graph that does not run green nodes again to check them: a report that
/// is answered from its first dependencies is not one it can check.
fn unverified() -> Graph {
    let runtime = Runtime::new(
        registry(),
        (nothing, not_overlaid),
        (Workspace::new(""), false),
    );
    let options = GraphOptions {
        verify: 0,
        ..GraphOptions::default()
    };
    Graph::in_memory(options.data(Arc::new(runtime))).unwrap()
}

#[test]
fn state_report_keeps_the_body_that_quiet_report_gives() {
    let mut graph = unverified();
    let variant = set_judged(&mut graph, RULE, judged("{}")).unwrap();
    set_run(&mut graph, "one".to_owned()).unwrap();
    assert_eq!(quiet_report(&graph, "all").unwrap(), None);

    let reads = vec![(RULE.to_owned(), String::new(), variant)];
    state_report(&graph, "all", (reads, "body".to_owned())).unwrap();

    assert_eq!(
        quiet_report(&graph, "all").unwrap().as_deref(),
        Some("body")
    );
    assert_eq!(quiet_report(&graph, "other").unwrap(), None);
}

#[test]
fn set_run_to_another_state_leaves_no_report_to_give() {
    let mut graph = unverified();
    set_run(&mut graph, "one".to_owned()).unwrap();
    state_report(&graph, "all", (Vec::new(), "body".to_owned())).unwrap();

    set_run(&mut graph, "one".to_owned()).unwrap();
    assert!(quiet_report(&graph, "all").unwrap().is_some());
    set_run(&mut graph, "two".to_owned()).unwrap();
    assert_eq!(quiet_report(&graph, "all").unwrap(), None);
}
