//! The model on the graph against the model of a merge: after every edit of a
//! random sequence the project the queries give is the one `Project::merge`
//! makes of the same fragments, and what a file's keys follow is what its
//! neighbors can see.

use lighthouse_graph::{Error, Graph, Options, Query};
use lighthouse_model::{
    Position, Span,
    ucm::{
        Edge, EdgeKind, File, Fragment, FunctionSummary, Held, Model, Module, NOTICES, Node,
        PROJECT, Place, Project, Resolution, Symbol, SymbolId, SymbolKind, Target, View,
        Visibility, stated_files,
    },
};

use crate::graph::set_units;

/// A tiny deterministic generator.
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
    let start = Position { line, col: 1 };
    Span {
        start,
        end: Position { line, col: 9 },
    }
}

/// What a file declares: functions by name, the calls they make by target,
/// the modules it imports and its line shift.
#[derive(Clone, Default, PartialEq)]
struct Spec {
    lang: &'static str,
    module: String,
    functions: Vec<String>,
    calls: Vec<(String, String)>,
    imports: Vec<String>,
    shift: u32,
    statements: u32,
    test: bool,
}

impl Spec {
    fn path(&self, at: usize) -> String {
        let ext = if self.lang == "go" { "go" } else { "rs" };
        format!("p{at}.{ext}")
    }

    fn fragment(&self, path: &str) -> Fragment {
        let id = |name: &str| SymbolId::new(&self.module, &[], name, SymbolKind::Function);
        let symbols: Vec<Symbol> = self
            .functions
            .iter()
            .map(|name| {
                Symbol::new(
                    id(name),
                    SymbolKind::Function,
                    Visibility::Public,
                    path.into(),
                    name.clone(),
                )
            })
            .collect();
        let places: Vec<Place> = (0..self.functions.len())
            .map(|n| Place::new(id(&self.functions[n]), at(n as u32 + 1 + self.shift)))
            .collect();
        let call = |from: &str, to: &str| Edge {
            kind: EdgeKind::Calls,
            from: Node::Symbol(id(from)),
            to: Target::Path(to.to_owned()),
            resolution: Resolution::Syntactic,
            site: Some(at(40 + self.shift)),
            when: Default::default(),
        };
        let mut edges: Vec<Edge> = self.calls.iter().map(|(f, t)| call(f, t)).collect();
        edges.extend(self.imports.iter().map(|to| Edge {
            kind: EdgeKind::Imports,
            from: Node::Module(self.module.clone()),
            to: Target::Path(to.clone()),
            resolution: Resolution::Syntactic,
            site: Some(at(1 + self.shift)),
            when: Default::default(),
        }));
        Fragment {
            files: vec![File {
                path: path.into(),
                lang: self.lang.to_owned(),
                generated: false,
                test: self.test,
                facts: Default::default(),
            }],
            modules: vec![Module {
                path: self.module.clone(),
                name: None,
                test_of: None,
                language: None,
                example: false,
            }],
            functions: self
                .functions
                .iter()
                .map(|name| summary(id(name), self.statements))
                .collect(),
            symbols,
            places,
            edges,
            ..Fragment::default()
        }
    }
}

fn summary(symbol: SymbolId, statements: u32) -> FunctionSummary {
    FunctionSummary {
        symbol,
        max_nesting: 0,
        statements,
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

/// The graph of the world: every file a unit of its own.
struct World {
    graph: Graph,
    specs: Vec<Spec>,
}

impl World {
    fn new(specs: Vec<Spec>) -> Self {
        let graph = Graph::in_memory(Options::default()).unwrap();
        let mut world = Self { graph, specs };
        world.set();
        world
    }

    /// Sets what the host says and the providers answered for the specs.
    fn set(&mut self) {
        let fragments: Vec<Fragment> = self
            .specs
            .iter()
            .enumerate()
            .map(|(at, spec)| spec.fragment(&spec.path(at)))
            .collect();
        set_units(&mut self.graph, &fragments).unwrap();
    }

    fn project(&self) -> Project {
        (*self.graph.query(&PROJECT, &()).unwrap()).clone()
    }

    /// What the view reads of the graph is what the merge says of every symbol
    /// and file: the shape, the place, the summary and the test flag.
    fn agrees(&self, at: &str) {
        let held = Held::default();
        let view = View::new(&self.graph, &held).unwrap();
        let merged = self.merged();
        for symbol in merged.symbols() {
            let id = &symbol.id;
            assert_eq!(view.symbol(id), Some(symbol), "{at}: symbol {id:?}");
            assert_eq!(view.place(id), merged.place(id), "{at}: place {id:?}");
            assert_eq!(
                view.function(id),
                merged.function(id),
                "{at}: function {id:?}"
            );
            assert_eq!(view.in_test(id), merged.in_test(id), "{at}: in_test {id:?}");
        }
        for file in merged.files() {
            assert_eq!(
                view.file(&file.path),
                Some(file),
                "{at}: file {:?}",
                file.path
            );
        }
        let edges: Vec<_> = view.edges().collect();
        assert_eq!(edges, merged.edges().collect::<Vec<_>>(), "{at}: edges");
        for from in 0..edges.len() {
            assert_eq!(
                view.origin(from),
                merged.origin(from),
                "{at}: origin {from}"
            );
        }
        assert_eq!(
            *self.graph.query(&NOTICES, &()).unwrap(),
            merged.notices(),
            "{at}: notices"
        );
        let mut stated = stated_files(&self.graph).unwrap();
        stated.sort_by(|a, b| a.0.path.cmp(&b.0.path));
        let mut files: Vec<_> = merged
            .files()
            .map(|file| {
                let tested = file.test
                    || merged
                        .symbols_in(&file.path)
                        .any(|symbol| merged.in_test(&symbol.id));
                (file.clone(), tested)
            })
            .collect();
        files.sort_by(|a, b| a.0.path.cmp(&b.0.path));
        assert_eq!(stated, files, "{at}: stated files");
    }

    fn merged(&self) -> Project {
        Project::merge(
            self.specs
                .iter()
                .enumerate()
                .map(|(at, spec)| spec.fragment(&spec.path(at))),
        )
    }
}

fn spec(module: &str, functions: &[&str]) -> Spec {
    Spec {
        lang: "rust",
        module: module.to_owned(),
        functions: functions.iter().map(|f| (*f).to_owned()).collect(),
        statements: 1,
        ..Spec::default()
    }
}

/// A small world: `a` calls `b`, and each is a file.
fn two() -> Vec<Spec> {
    let mut a = spec("a", &["fa"]);
    a.calls.push(("fa".to_owned(), "b::fb".to_owned()));
    vec![a, spec("b", &["fb"])]
}

fn random(rng: &mut Rng, specs: &mut Vec<Spec>) {
    let at = rng.next(specs.len());
    let names = ["x", "y", "z", "w"];
    let name = names[rng.next(names.len())].to_owned();
    let target = format!("m{}::{}", rng.next(5), names[rng.next(names.len())]);
    match rng.next(12) {
        0 => specs[at].shift += 1,
        1 => specs[at].statements += 1,
        2 => {
            if !specs[at].functions.contains(&name) {
                specs[at].functions.push(name);
            }
        }
        3 => {
            specs[at].functions.retain(|f| *f != name);
            specs[at].calls.retain(|(from, _)| *from != name);
        }
        4 => {
            if let Some(from) = specs[at].functions.first().cloned() {
                specs[at].calls.push((from, target));
            }
        }
        5 => {
            specs[at].calls.pop();
        }
        6 => specs[at].imports.push(format!("m{}", rng.next(5))),
        7 => {
            specs[at].imports.pop();
        }
        8 => {
            let module = format!("m{}", rng.next(5));
            specs.push(spec(&module, &[&name]));
        }
        9 if specs.len() > 2 => {
            specs.remove(at);
        }
        11 => specs[at].test = !specs[at].test,
        _ => specs[at].lang = if specs[at].lang == "go" { "rust" } else { "go" },
    }
}

#[test]
fn the_project_the_queries_give_is_the_project_a_merge_makes_after_every_edit() {
    let mut noticed = 0;
    for seed in 1..=6 {
        let mut rng = Rng(seed * 7919);
        let mut specs: Vec<Spec> = (0..5)
            .map(|n| spec(&format!("m{n}"), &["x", "y"]))
            .collect();
        specs[0].calls.push(("x".to_owned(), "m1::y".to_owned()));
        specs[2].calls.push(("y".to_owned(), "x".to_owned()));
        let mut world = World::new(specs.clone());
        let mut history = vec![specs.clone()];
        for step in 0..60 {
            if step % 9 == 8 {
                history.pop();
                specs = history.last().cloned().unwrap_or(specs);
            } else {
                random(&mut rng, &mut specs);
                history.push(specs.clone());
            }
            world.specs.clone_from(&specs);
            world.set();
            let project = world.project();
            assert_eq!(project, world.merged(), "seed {seed} step {step}");
            world.agrees(&format!("seed {seed} step {step}"));
            noticed += usize::from(!project.notices().is_empty());
        }
    }
    assert!(noticed > 0, "no sequence made a duplicate or an ambiguity");
}

/// An edit of the specs of a world.

#[test]
fn a_body_edit_runs_no_resolution_and_the_symbol_table_stays_equal() {
    let mut world = World::new(two());
    world.project();
    world.graph.take_executed();
    world.specs[1].statements = 9;
    world.set();
    world.project();
    let ran = world.graph.take_executed();
    let count = |kind: &str| ran.iter().filter(|label| label.starts_with(kind)).count();
    assert_eq!(count("resolved("), 1, "{ran:?}");
    assert_eq!(count("declarations("), 1, "{ran:?}");
    assert_eq!(count("symbols("), 0, "{ran:?}");
    assert_eq!(
        count("incoming("),
        0,
        "nothing it is tied to changed: {ran:?}"
    );
}

/// What a decision over the whole project reads of the model: the edges and
/// where the symbols are.
static READER: Query<(), (usize, usize)> = Query::new("reader", read);

fn read(graph: &Graph, _: &()) -> Result<(usize, usize), Error> {
    let held = Held::default();
    let view = View::new(graph, &held)?;
    let placed = view
        .symbols()
        .filter(|s| view.place(&s.id).is_some())
        .count();
    Ok((view.edges().count(), placed))
}

#[test]
fn what_reads_the_edges_and_the_places_of_the_project_runs_when_one_moves_only() {
    let mut world = World::new(two());
    let pull = |world: &World| {
        world.graph.take_executed();
        world.graph.query(&READER, &()).unwrap();
        world.graph.take_executed()
    };
    pull(&world);

    world.specs[1].statements = 9;
    world.set();
    let ran = pull(&world);
    assert!(
        !ran.iter().any(|label| label.starts_with("reader(")),
        "a body that moves no line: {ran:?}"
    );

    world.specs[1].shift = 3;
    world.set();
    let ran = pull(&world);
    assert!(
        ran.iter().any(|label| label.starts_with("reader(")),
        "the places moved: {ran:?}"
    );

    world.specs[0].imports.push("b".to_owned());
    world.set();
    let ran = pull(&world);
    assert!(
        ran.iter().any(|label| label.starts_with("reader(")),
        "an edge was added: {ran:?}"
    );
}
