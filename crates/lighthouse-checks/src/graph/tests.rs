//! The facts of the graph against the facts of a merge: after every edit of a
//! random sequence, each subject's fact is what the builder makes of the same
//! fragments merged into a project.

use std::collections::BTreeSet;

use super::kinds::{CONVENTIONS, Conventions, Pull, READS};
use crate::{
    fact::{Built, FACT, FUNCTION, Fact, SYMBOL, TEST, subject_key},
    local::Local,
};
use lighthouse_graph::{Durability, Graph, Options};
use lighthouse_model::ucm::Host;
use lighthouse_model::{
    Position, Span,
    ucm::{
        Edge, EdgeKind, File, Fragment, FunctionSummary, Model, Module, Node, Project, Resolution,
        Symbol, SymbolId, SymbolKind, TestCase, TestStyle, Visibility,
    },
};
use std::sync::Arc;

use lighthouse_plugin::{ANALYSIS, Analyzers, Plugin, Queries, Reads, Workspace};

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
}

impl Spec {
    fn path(&self, at: usize) -> String {
        format!("p{at}.{}", if self.lang == "go" { "go" } else { "rs" })
    }

    fn fragment(&self, path: &str) -> Fragment {
        let id = |name: &str, kind| SymbolId::new(&self.module, &[], name, kind);
        let ty = id("T", SymbolKind::Type);
        let mut symbols = Vec::new();
        let mut functions = Vec::new();
        let mut line = self.shift;
        let mut push = |symbol: Symbol| {
            line += 1;
            symbols.push(Symbol {
                span: at(line),
                ..symbol
            });
        };
        for name in &self.functions {
            let id = id(name, SymbolKind::Function);
            functions.push(summary(id.clone()));
            push(Symbol::new(
                id,
                SymbolKind::Function,
                Visibility::Private,
                path.into(),
                at(0),
                name.clone(),
            ));
        }
        if !self.methods.is_empty() {
            push(Symbol::new(
                ty.clone(),
                SymbolKind::Type,
                Visibility::Public,
                path.into(),
                at(0),
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
                    at(0),
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
                at(0),
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
                hash: format!("{}", self.shift),
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
            functions,
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
        implementation: false,
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

/// The analyzers of the graph: the metrics, over files that have no text.
fn analyzers() -> Arc<Analyzers> {
    let by_id = Metrics
        .analyzers()
        .into_iter()
        .map(|analyzer| (analyzer.manifest().id.clone(), Arc::from(analyzer)))
        .collect();
    Arc::new(Analyzers::new(by_id, |_, _| Ok(Arc::new(None))))
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
        for kind in [SYMBOL, FUNCTION, TEST] {
            graph
                .set_input(&READS, &kind.to_owned(), everything(), Durability::Stable)
                .unwrap();
        }
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
            let ids: BTreeSet<&SymbolId> = project.symbols().map(|s| &s.id).collect();
            for id in ids {
                for kind in [SYMBOL, FUNCTION, TEST] {
                    let key = subject_key(kind, id.as_str());
                    let want = local.pull(FACT, &key).unwrap();
                    let got = pull.pull(FACT, &key).unwrap();
                    let (want, got) = (
                        want.downcast::<Built>().unwrap(),
                        got.downcast::<Built>().unwrap(),
                    );
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
        let key = subject_key(SYMBOL, symbol.id.as_str());
        let narrow = narrow
            .pull(FACT, &key)
            .unwrap()
            .downcast::<Built>()
            .unwrap();
        let wide = wide.pull(FACT, &key).unwrap().downcast::<Built>().unwrap();
        let (Some(Fact::Map(narrow)), Some(Fact::Map(wide))) = (narrow.fact(), wide.fact()) else {
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

fn specs_path(at: usize) -> String {
    format!("p{at}.rs")
}
