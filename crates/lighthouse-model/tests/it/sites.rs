use lighthouse_model::{
    Edge, EdgeKind, File, Fragment, Node, Position, Project, Resolution, Span, Symbol, SymbolId,
    SymbolKind, Target, Visibility,
};

fn span(line: u32, col: u32) -> Span {
    Span {
        start: Position { line, col },
        end: Position { line, col: col + 3 },
    }
}

fn symbol(name: &str, file: &str) -> Symbol {
    Symbol {
        id: SymbolId::new("m", &[], name, SymbolKind::Function),
        kind: SymbolKind::Function,
        visibility: Visibility::Private,
        owner: None,
        file: file.into(),
        span: span(1, 1),
        extent: None,
        doc: None,
        name: name.to_owned(),
        role: None,
        optional: false,
        type_ref: None,
    }
}

fn call(from: &Symbol, to: &str, site: Option<Span>) -> Edge {
    Edge {
        kind: EdgeKind::Calls,
        from: Node::Symbol(from.id.clone()),
        to: Target::Path(format!("m::{to}")),
        resolution: Resolution::Semantic,
        site,
    }
}

fn fragment(file: &str, symbols: Vec<Symbol>, edges: Vec<Edge>) -> Fragment {
    Fragment {
        files: vec![File {
            path: file.into(),
            lang: "x".to_owned(),
            hash: String::new(),
            generated: false,
            test: false,
        }],
        symbols,
        edges,
        ..Fragment::default()
    }
}

#[test]
fn every_site_of_a_reference_is_kept_though_the_edge_is_one() {
    let (target, a, b) = (symbol("t", "t.x"), symbol("a", "a.x"), symbol("b", "b.x"));
    let project = Project::merge([
        fragment("t.x", vec![target.clone()], vec![]),
        fragment(
            "a.x",
            vec![a.clone()],
            vec![
                call(&a, "t", Some(span(3, 5))),
                call(&a, "t", Some(span(7, 9))),
            ],
        ),
        fragment(
            "b.x",
            vec![b.clone()],
            vec![call(&b, "t", Some(span(2, 2)))],
        ),
    ]);

    let sites = project.sites(&target.id);

    let at: Vec<_> = sites
        .iter()
        .map(|s| {
            (
                s.file.to_str().unwrap(),
                s.span.start.line,
                s.span.start.col,
            )
        })
        .collect();
    assert_eq!(at, [("a.x", 3, 5), ("a.x", 7, 9), ("b.x", 2, 2)]);
    assert!(sites.iter().all(|s| s.kind == EdgeKind::Calls));
    assert_eq!(
        project.callers(&target.id).len(),
        2,
        "callers are distinct symbols"
    );
    let edges = project
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls && e.from == Node::Symbol(a.id.clone()))
        .count();
    assert_eq!(edges, 1, "the same relation is one edge");
}

#[test]
fn edges_without_a_site_or_a_resolved_symbol_have_no_site() {
    let (target, a) = (symbol("t", "t.x"), symbol("a", "a.x"));
    let project = Project::merge([
        fragment("t.x", vec![target.clone()], vec![]),
        fragment(
            "a.x",
            vec![a.clone()],
            vec![call(&a, "t", None), call(&a, "missing", Some(span(1, 1)))],
        ),
    ]);

    assert!(project.sites(&target.id).is_empty());
}
