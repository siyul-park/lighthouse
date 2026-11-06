//! The model as the graph holds it, read through a view.

use lighthouse_graph::{Graph, Options};
use lighthouse_model::{
    Position, Span,
    ucm::{
        File, Fragment, Held, Host, Model, Place, Symbol, SymbolId, SymbolKind, TestCase,
        TestStyle, View, Visibility,
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
