//! The model as the graph holds it, read through a view.

use std::sync::atomic::{AtomicUsize, Ordering};

use lighthouse_graph::{Error, Graph, Options, Query};
use lighthouse_model::{
    Position, Span,
    ucm::{
        File, Fragment, Held, Host, Model, Place, Symbol, SymbolId, SymbolKind, TestCase,
        TestStyle, View, Visibility, register,
    },
};

fn fragment(module: &str, name: &str, test: bool) -> Fragment {
    let at = Position { line: 1, col: 1 };
    let kind = if test {
        SymbolKind::Test
    } else {
        SymbolKind::Function
    };
    let id = SymbolId::new(module, &[], name, kind);
    let path = format!("{module}/{name}.go");
    let symbol = Symbol::new(
        id.clone(),
        kind,
        Visibility::Public,
        path.clone().into(),
        name,
    );
    Fragment {
        files: vec![File {
            path: path.into(),
            lang: "go".to_owned(),
            generated: false,
            test,
            facts: Default::default(),
        }],
        symbols: vec![symbol],
        places: vec![Place::new(id.clone(), Span { start: at, end: at })],
        tests: test
            .then(|| TestCase {
                symbol: id,
                nesting: 0,
                style: TestStyle::Table,
                targets: Vec::new(),
            })
            .into_iter()
            .collect(),
        ..Fragment::default()
    }
}

fn graph(parts: &[Fragment]) -> Graph {
    let mut graph = Graph::in_memory(Options::default()).unwrap();
    Host::set_units(&mut graph, parts).unwrap();
    graph
}

#[test]
fn view() {
    let graph = graph(&[fragment("a", "f", false), fragment("a", "t", true)]);
    let held = Held::default();
    let view = View::new(&graph, &held).unwrap();
    let id = SymbolId::new("a", &[], "f", SymbolKind::Function);

    assert_eq!(view.symbol(&id).map(|s| s.name.as_str()), Some("f"));
    assert_eq!(view.symbols().count(), 2);
    assert_eq!(view.tests_in("a").count(), 1);
    assert!(view.file("a/f.go".as_ref()).is_some());
}

#[test]
fn view_new() {
    let graph = graph(&[]);
    let held = Held::default();

    assert!(View::new(&graph, &held).is_ok());
}

#[test]
fn view_failure() {
    let graph = graph(&[fragment("a", "f", false)]);
    let held = Held::default();
    let view = View::new(&graph, &held).unwrap();

    assert_eq!(view.symbols().count(), 1);
    assert!(view.failure().is_none());
}

static RUNS: AtomicUsize = AtomicUsize::new(0);
static SYMBOL_COUNT: Query<(), usize> = Query::new("symbol_count", symbol_count);

fn symbol_count(graph: &Graph, _: &()) -> Result<usize, Error> {
    RUNS.fetch_add(1, Ordering::SeqCst);
    let held = Held::default();
    let view = View::new(graph, &held)?;
    Ok(view.symbols().count())
}

#[test]
fn view_symbols_ignore_places() {
    let mut graph = graph(&[fragment("a", "f", false), fragment("b", "g", false)]);
    SYMBOL_COUNT.register(&graph).unwrap();
    assert_eq!(*graph.query(&SYMBOL_COUNT, &()).unwrap(), 2);

    let mut moved = fragment("b", "g", false);
    let at = Position { line: 5, col: 1 };
    moved.places[0].span = Span { start: at, end: at };
    Host::set_units(&mut graph, &[fragment("a", "f", false), moved]).unwrap();

    assert_eq!(*graph.query(&SYMBOL_COUNT, &()).unwrap(), 2);
    assert_eq!(RUNS.load(Ordering::SeqCst), 1);
}

static PLACED: Query<(), bool> = Query::new("placed", placed);

fn placed(graph: &Graph, _: &()) -> Result<bool, Error> {
    let held = Held::default();
    let view = View::new(graph, &held)?;
    let id = SymbolId::new("a", &[], "f", SymbolKind::Function);
    Ok(view.place(&id).is_some())
}

fn reopened(path: &std::path::Path, parts: &[Fragment]) -> Graph {
    let mut graph = Graph::open(path, Options::default()).unwrap();
    Host::set_units(&mut graph, parts).unwrap();
    graph
}

#[test]
fn register_lets_a_stored_node_that_read_places_be_validated_before_any_is_pulled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("graph.db");
    let mut first = reopened(
        &path,
        &[fragment("a", "f", false), fragment("b", "g", false)],
    );
    first.query(&PLACED, &()).unwrap();
    first.commit().unwrap();
    drop(first);

    let mut moved = fragment("b", "g", false);
    let at = Position { line: 5, col: 1 };
    moved.places[0].span = Span { start: at, end: at };
    let second = reopened(&path, &[fragment("a", "f", false), moved]);
    register(&second).unwrap();
    second.query(&PLACED, &()).unwrap();

    let ran = second.take_executed();
    assert!(
        !ran.iter().any(|label| label.starts_with("placed(")),
        "{ran:?}"
    );
}
