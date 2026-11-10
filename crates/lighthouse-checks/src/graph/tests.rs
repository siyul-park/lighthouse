//! The facts of the graph against the facts of a merge: after every edit of a
//! random sequence, each subject's fact is what the builder makes of the same
//! fragments merged into a project.

use std::collections::BTreeSet;

use super::kinds::{CONVENTIONS, Conventions, Pull, READS};
use crate::{
    fact::{COMMENT, FACT, FUNCTION, Fact, Facts, SYMBOL, TEST, class_of, fact_key},
    local::Local,
};
use lighthouse_graph::{Durability, Graph, Options};
use lighthouse_model::ucm::{Comment, Host};
use lighthouse_model::{
    Position, Span,
    ucm::{
        Edge, EdgeKind, File, Fragment, FunctionSummary, Model, Module, Node, Place, Project,
        Resolution, Symbol, SymbolId, SymbolKind, TestCase, TestStyle, Visibility,
    },
};
use lighthouse_spec::{Content, OVERLAY};
use std::sync::Arc;

use lighthouse_plugin::{ANALYSIS, Queries, Reads, Registry, Runtime, Workspace};

use crate::metrics::{MEASURES, Metrics};

struct Rng(u64);

impl Rng {
    fn next(&mut self, below: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % below as u64) as usize
    }
}

fn at(line: u32) -> Span {
    Span {
        start: Position { line, col: 1 },
        end: Position { line, col: 9 },
    }
}

/// What a file declares: functions, the methods of one type, a test, the
/// calls they make by target and the modules it imports.
#[derive(Clone, Default)]
struct Spec {
    lang: &'static str,
    module: String,
    functions: Vec<String>,
    methods: Vec<String>,
    tested: bool,
    calls: Vec<(String, String)>,
    imports: Vec<String>,
    shift: u32,
    bulk: u32,
}

impl Spec {
    fn path(&self, at: usize) -> String {
        format!("p{at}.{}", if self.lang == "go" { "go" } else { "rs" })
    }

    fn fragment(&self, path: &str) -> Fragment {
        let id = |name: &str, kind| SymbolId::new(&self.module, &[], name, kind);
        let ty = id("T", SymbolKind::Type);
        let mut symbols = Vec::new();
        let mut places = Vec::new();
        let mut functions = Vec::new();
        let mut line = self.shift;
        let mut push = |symbol: Symbol| {
            line += 1;
            places.push(Place::new(symbol.id.clone(), at(line)));
            symbols.push(symbol);
        };
        for name in &self.functions {
            let id = id(name, SymbolKind::Function);
            functions.push(summary(id.clone()));
            push(Symbol::new(
                id,
                SymbolKind::Function,
                Visibility::Private,
                path.into(),
                name.clone(),
            ));
        }
        if !self.methods.is_empty() {
            push(Symbol::new(
                ty.clone(),
                SymbolKind::Type,
                Visibility::Public,
                path.into(),
                "T",
            ));
        }
        for name in &self.methods {
            let id = SymbolId::new(&self.module, &["T"], name, SymbolKind::Method);
            functions.push(summary(id.clone()));
            push(Symbol {
                owner: Some(ty.clone()),
                ..Symbol::new(
                    id,
                    SymbolKind::Method,
                    Visibility::Public,
                    path.into(),
                    name.clone(),
                )
            });
        }
        let mut tests = Vec::new();
        if self.tested {
            let test = id("test_a", SymbolKind::Test);
            functions.push(summary(test.clone()));
            tests.push(TestCase {
                symbol: test.clone(),
                nesting: 0,
                style: TestStyle::Scenario,
                targets: Vec::new(),
            });
            push(Symbol::new(
                test,
                SymbolKind::Test,
                Visibility::Private,
                path.into(),
                "test_a",
            ));
        }
        let call = |from: &str, to: &str| Edge {
            kind: EdgeKind::Calls,
            from: Node::Symbol(id(from, SymbolKind::Function)),
            to: lighthouse_model::ucm::Target::Path(to.to_owned()),
            resolution: Resolution::Syntactic,
            site: Some(at(50 + self.shift)),
            when: Default::default(),
        };
        let mut edges: Vec<Edge> = self.calls.iter().map(|(f, t)| call(f, t)).collect();
        edges.extend(self.imports.iter().map(|to| Edge {
            kind: EdgeKind::Imports,
            from: Node::Module(self.module.clone()),
            to: lighthouse_model::ucm::Target::Path(to.clone()),
            resolution: Resolution::Syntactic,
            site: Some(at(1)),
            when: Default::default(),
        }));
        Fragment {
            files: vec![File {
                path: path.into(),
                lang: self.lang.to_owned(),
                generated: false,
                test: false,
                facts: Default::default(),
            }],
            modules: vec![Module {
                path: self.module.clone(),
                name: None,
                test_of: None,
                language: None,
                example: false,
            }],
            symbols,
            places,
            functions: functions
                .into_iter()
                .map(|f| FunctionSummary {
                    statements: f.statements + self.bulk,
                    ..f
                })
                .collect(),
            tests,
            edges,
            ..Fragment::default()
        }
    }
}

fn summary(symbol: SymbolId) -> FunctionSummary {
    FunctionSummary {
        symbol,
        max_nesting: 0,
        statements: 1,
        top_level: 1,
        params: 0,
        returns: 0,
        tokens: 3,
        flow: Vec::new(),
        forwards_to: None,
        signature: Default::default(),
        events: Vec::new(),
        manual_assertions: 0,
        constructs: false,
        configures: None,
    }
}

fn spec(module: &str, functions: &[&str]) -> Spec {
    Spec {
        lang: "rust",
        module: module.to_owned(),
        functions: functions.iter().map(|f| (*f).to_owned()).collect(),
        ..Spec::default()
    }
}

fn random(rng: &mut Rng, specs: &mut Vec<Spec>) {
    let at = rng.next(specs.len());
    let names = ["x", "y", "z", "w"];
    let name = names[rng.next(names.len())].to_owned();
    let target = format!("m{}::{}", rng.next(5), names[rng.next(names.len())]);
    match rng.next(14) {
        0 => specs[at].shift += 1,
        1 => {
            if !specs[at].functions.contains(&name) {
                specs[at].functions.push(name);
            }
        }
        2 => {
            specs[at].functions.retain(|f| *f != name);
            specs[at].calls.retain(|(from, _)| *from != name);
        }
        3 => {
            if let Some(from) = specs[at].functions.first().cloned() {
                specs[at].calls.push((from, target));
            }
        }
        4 => {
            specs[at].calls.pop();
        }
        5 => specs[at].imports.push(format!("m{}", rng.next(5))),
        6 => {
            specs[at].imports.pop();
        }
        7 => {
            if !specs[at].methods.contains(&name) {
                specs[at].methods.push(name);
            }
        }
        8 => {
            specs[at].methods.retain(|m| *m != name);
        }
        9 => specs[at].tested = !specs[at].tested,
        10 => {
            let module = format!("m{}", rng.next(5));
            specs.push(spec(&module, &[&name]));
        }
        11 if specs.len() > 2 => {
            specs.remove(at);
        }
        _ => specs[at].lang = if specs[at].lang == "go" { "rust" } else { "go" },
    }
}

/// Every field a fact can carry, for every sort of subject.
fn everything() -> Reads {
    Reads {
        dynamic: true,
        related_dynamic: true,
        ..Reads::default()
    }
}

/// What the graph runs: the metrics, over files that have no text.
fn analyzers() -> Arc<Runtime> {
    let mut registry = Registry::default();
    registry.register(&Metrics).unwrap();
    Arc::new(Runtime::new(
        Arc::new(registry),
        (|_, _| Ok(Arc::new(None)), |_, _| Ok(None)),
        (Workspace::new(""), false),
    ))
}

fn fragments(specs: &[Spec]) -> Vec<Fragment> {
    specs
        .iter()
        .enumerate()
        .map(|(at, spec)| spec.fragment(&spec.path(at)))
        .collect()
}

#[test]
fn the_fact_of_a_subject_is_what_the_builder_makes_of_a_merge_after_every_edit() {
    let ws = Workspace::new(".");
    let (mut compared, mut present) = (0, 0);
    for seed in 1..=4 {
        let mut rng = Rng(seed * 7919);
        let mut specs: Vec<Spec> = (0..5)
            .map(|n| {
                let mut s = spec(&format!("m{n}"), &["x", "y"]);
                s.methods = vec!["x".to_owned(), "y".to_owned()];
                s.tested = n % 2 == 0;
                s
            })
            .collect();
        specs[0].calls.push(("x".to_owned(), "m1::y".to_owned()));
        specs[2].calls.push(("y".to_owned(), "x".to_owned()));
        let mut graph = Graph::in_memory(Options::default().data(analyzers())).unwrap();
        graph
            .set_input(
                &READS,
                &class_of(&everything()),
                everything(),
                Durability::Stable,
            )
            .unwrap();
        graph
            .set_input(
                &CONVENTIONS,
                &(),
                Conventions::default(),
                Durability::Stable,
            )
            .unwrap();
        for step in 0..30 {
            random(&mut rng, &mut specs);
            let parts = fragments(&specs);
            Host::set_units(&mut graph, &parts).unwrap();
            let project = Project::merge(parts);
            let local = Local::new(
                &project,
                &ws,
                [
                    (SYMBOL.to_owned(), everything()),
                    (FUNCTION.to_owned(), everything()),
                    (TEST.to_owned(), everything()),
                ],
            );
            let pull = Pull::new(&graph);
            let files: BTreeSet<String> = project
                .files()
                .map(|f| f.path.to_string_lossy().into_owned())
                .collect();
            for file in files {
                for kind in [SYMBOL, FUNCTION, TEST] {
                    let key = fact_key(kind, &class_of(&everything()), &file);
                    let want = local.pull(FACT, &key).unwrap();
                    let got = pull.pull(FACT, &key).unwrap();
                    let (want, got) = (
                        want.downcast::<Facts>().unwrap(),
                        got.downcast::<Facts>().unwrap(),
                    );
                    for symbol in project.symbols_in(file.as_ref()) {
                        let id = symbol.id.as_str();
                        let (want, got) = (want.get(id).unwrap(), got.get(id).unwrap());
                        assert_eq!(
                            got.fact(),
                            want.fact(),
                            "seed {seed} step {step} {kind} {id:?}"
                        );
                        compared += 1;
                        present += usize::from(got.fact().is_some());
                    }
                }
            }
        }
    }
    assert!(compared > 1000, "{compared} facts were compared");
    assert!(present > 500, "only {present} of them were facts");
}

#[test]
fn a_rule_sees_the_same_fields_when_another_rule_asks_for_more() {
    let ws = Workspace::new(".");
    let project = Project::merge(fragments(&[spec("m0", &["x", "y"]), spec("m1", &["z"])]));
    let few = Reads {
        fields: ["name".to_owned(), "kind".to_owned()].into(),
        ..Reads::default()
    };
    let narrow = Local::new(&project, &ws, [(SYMBOL.to_owned(), few.clone())]);
    let wide = Local::new(&project, &ws, [(SYMBOL.to_owned(), everything())]);
    let mut seen = 0;

    for symbol in project.symbols() {
        let file = symbol.file.to_string_lossy();
        let narrow_key = fact_key(SYMBOL, &class_of(&few), &file);
        let wide_key = fact_key(SYMBOL, &class_of(&everything()), &file);
        let narrow = narrow
            .pull(FACT, &narrow_key)
            .unwrap()
            .downcast::<Facts>()
            .unwrap();
        let wide = wide
            .pull(FACT, &wide_key)
            .unwrap()
            .downcast::<Facts>()
            .unwrap();
        let id = symbol.id.as_str();
        let (Some(Fact::Map(narrow)), Some(Fact::Map(wide))) = (
            narrow.get(id).and_then(|built| built.fact()),
            wide.get(id).and_then(|built| built.fact()),
        ) else {
            panic!("{symbol:?} has no fact");
        };
        for (field, value) in narrow {
            assert_eq!(wide.get(field), Some(value), "{field} of {:?}", symbol.id);
        }
        assert!(wide.len() > narrow.len());
        seen += 1;
    }

    assert!(seen > 0);
}

#[test]
fn an_edit_runs_the_analysis_of_the_file_it_changed_and_of_no_other() {
    let mut specs = vec![spec("m0", &["x"]), spec("m1", &["y"])];
    let mut graph = Graph::in_memory(Options::default().data(analyzers())).unwrap();
    let pull = |graph: &Graph| -> Vec<String> {
        (0..2)
            .map(|at| {
                let key = (MEASURES.to_owned(), specs_path(at));
                (*graph.query(&ANALYSIS, &key).unwrap()).clone()
            })
            .collect()
    };
    Host::set_units(&mut graph, &fragments(&specs)).unwrap();
    let before = pull(&graph);
    graph.take_executed();

    specs[1].shift = 3;
    Host::set_units(&mut graph, &fragments(&specs)).unwrap();
    let after = pull(&graph);
    let ran = graph.take_executed();

    assert_eq!(before[0], after[0]);
    assert!(ran.iter().any(|label| label.contains("p1.rs")), "{ran:?}");
    assert!(
        !ran.iter()
            .any(|label| label.contains("analysis") && label.contains("p0.rs")),
        "{ran:?}"
    );
}

#[test]
fn the_facts_of_a_comment_follow_the_text_of_its_file() {
    let mut graph = Graph::in_memory(Options::default()).unwrap();
    let mut part = Spec {
        shift: 2,
        ..spec("m0", &["x"])
    }
    .fragment("p0.rs");
    part.comments = vec![Comment {
        file: "p0.rs".into(),
        span: Span {
            start: Position { line: 1, col: 1 },
            end: Position { line: 1, col: 6 },
        },
        text: "// hi".to_owned(),
        attached_to: None,
    }];
    Host::set_units(&mut graph, &[part]).unwrap();
    let text = |graph: &mut Graph, text: &str| {
        let content = Some(Content::of_text(text));
        graph
            .set_input(&OVERLAY, &"p0.rs".to_owned(), content, Durability::Volatile)
            .unwrap();
    };
    let field = |graph: &Graph, name: &str| {
        let key = fact_key(COMMENT, "", "p0.rs");
        let facts = Pull::new(graph).pull(FACT, &key).unwrap();
        let facts = facts.downcast::<Facts>().unwrap();
        let Some(Fact::Map(map)) = facts.get("00000000").and_then(|built| built.fact()) else {
            panic!("no fact");
        };
        map[name].clone()
    };

    text(&mut graph, "// hi\nfn x() {}\n");
    assert_eq!(field(&graph, "standalone"), Fact::Bool(true));
    assert_eq!(field(&graph, "gap"), Fact::Int(0));

    text(&mut graph, "// hi\n\nfn x() {}\n");
    assert_eq!(field(&graph, "gap"), Fact::Int(1));
}

#[test]
fn a_body_edit_runs_the_facts_of_the_files_it_reaches_and_of_no_other() {
    let mut specs: Vec<Spec> = (0..5)
        .map(|n| {
            let mut s = spec(&format!("m{n}"), &["x", "y"]);
            s.methods = vec!["x".to_owned(), "y".to_owned()];
            s
        })
        .collect();
    specs[0].calls.push(("x".to_owned(), "m1::y".to_owned()));
    let mut graph = Graph::in_memory(Options::default().data(analyzers())).unwrap();
    let class = class_of(&everything());
    graph
        .set_input(&READS, &class, everything(), Durability::Stable)
        .unwrap();
    graph
        .set_input(
            &CONVENTIONS,
            &(),
            Conventions::default(),
            Durability::Stable,
        )
        .unwrap();
    let pull_all = |graph: &Graph| {
        let pull = Pull::new(graph);
        for at in 0..5 {
            for kind in [SYMBOL, FUNCTION, TEST] {
                let key = fact_key(kind, &class, &specs_path(at));
                pull.pull(FACT, &key).unwrap();
            }
        }
    };
    Host::set_units(&mut graph, &fragments(&specs)).unwrap();
    pull_all(&graph);
    graph.take_executed();

    specs[1].bulk = 1;
    Host::set_units(&mut graph, &fragments(&specs)).unwrap();
    pull_all(&graph);
    let ran = graph.take_executed();

    let facts: Vec<&String> = ran
        .iter()
        .filter(|label| label.starts_with("fact"))
        .collect();
    assert!(!facts.is_empty(), "{ran:?}");
    for label in facts {
        let reached = label.contains("p0.rs") || label.contains("p1.rs");
        assert!(reached, "{label}");
    }
    assert!(
        !ran.iter().any(|label| label.starts_with("symbols")),
        "{ran:?}"
    );
}

/// A graph with the inputs the facts read, over the units of `specs`.
fn primed(mut graph: Graph, specs: &[Spec]) -> Graph {
    let class = class_of(&everything());
    graph
        .set_input(&READS, &class, everything(), Durability::Stable)
        .unwrap();
    graph
        .set_input(
            &CONVENTIONS,
            &(),
            Conventions::default(),
            Durability::Stable,
        )
        .unwrap();
    Host::set_units(&mut graph, &fragments(specs)).unwrap();
    graph
}

fn pull_facts(graph: &Graph, files: usize) {
    let pull = Pull::new(graph);
    for at in 0..files {
        for kind in [SYMBOL, FUNCTION, TEST] {
            let key = fact_key(kind, &class_of(&everything()), &specs_path(at));
            pull.pull(FACT, &key).unwrap();
        }
    }
}

fn indexed(ran: &[String]) -> Vec<&String> {
    ran.iter()
        .filter(|label| label.starts_with("index") || label.starts_with("aggregate"))
        .collect()
}

#[test]
fn a_new_process_reads_the_indexes_from_the_store_and_runs_no_aggregate() {
    let specs: Vec<Spec> = (0..3)
        .map(|n| {
            let mut s = spec(&format!("m{n}"), &["x", "y"]);
            s.methods = vec!["x".to_owned(), "y".to_owned()];
            s
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("graph.db");
    let open = || {
        let options = Options::default().data(analyzers());
        primed(Graph::open(&path, options).unwrap(), &specs)
    };
    let mut graph = open();
    pull_facts(&graph, 3);
    assert!(!indexed(&graph.take_executed()).is_empty());
    graph.commit().unwrap();
    drop(graph);

    let graph = open();
    pull_facts(&graph, 3);

    let ran = graph.take_executed();
    assert!(ran.iter().any(|label| label.starts_with("fact")), "{ran:?}");
    assert_eq!(indexed(&ran), Vec::<&String>::new());
}

#[test]
fn a_revert_of_a_comment_edit_runs_no_fact_and_no_index() {
    let specs = vec![spec("m0", &["x"]), spec("m1", &["y"])];
    let mut graph = primed(
        Graph::in_memory(Options::default().data(analyzers())).unwrap(),
        &specs,
    );
    let text = |graph: &mut Graph, text: &str| {
        let content = Some(Content::of_text(text));
        graph
            .set_input(&OVERLAY, &"p0.rs".to_owned(), content, Durability::Volatile)
            .unwrap();
    };
    text(&mut graph, "fn x() {}\n");
    pull_facts(&graph, 2);
    text(&mut graph, "// note\nfn x() {}\n");
    pull_facts(&graph, 2);
    graph.take_executed();

    text(&mut graph, "fn x() {}\n");
    pull_facts(&graph, 2);

    let ran = graph.take_executed();
    let made: Vec<&String> = ran
        .iter()
        .filter(|label| label.starts_with("fact") || label.starts_with("index"))
        .collect();
    assert_eq!(made, Vec::<&String>::new());
}

fn specs_path(at: usize) -> String {
    format!("p{at}.rs")
}
