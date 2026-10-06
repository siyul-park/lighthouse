use lighthouse_model::{
    Edge, EdgeKind, Fragment, Node, Position, Project, Resolution, Span, Symbol, SymbolId,
    SymbolKind, Target, Visibility,
};

fn at() -> Span {
    let p = Position { line: 1, col: 1 };
    Span { start: p, end: p }
}

fn symbol(module: &str, name: &str) -> Symbol {
    Symbol {
        id: SymbolId::new(module, &[], name, SymbolKind::Function),
        kind: SymbolKind::Function,
        visibility: Visibility::Public,
        owner: None,
        span: at(),
        doc: None,
        name: name.to_owned(),
    }
}

fn call(from: &Symbol, to: &str) -> Edge {
    Edge {
        kind: EdgeKind::Calls,
        from: Node::Symbol(from.id.clone()),
        to: Target::Path(to.to_owned()),
        resolution: Resolution::Syntactic,
    }
}

#[test]
fn symbol_ids_are_qualified_and_stable() {
    let id = SymbolId::new("pkg", &["Owner"], "run", SymbolKind::Method);
    assert_eq!(id.as_str(), "pkg::Owner::run#method");
    assert_eq!(
        id,
        SymbolId::new("pkg", &["Owner"], "run", SymbolKind::Method)
    );
    assert_ne!(
        id,
        SymbolId::new("pkg", &["Owner"], "run", SymbolKind::Function)
    );
}

#[test]
fn merge_resolves_cross_fragment_targets_and_keeps_unknown_paths() {
    let a = symbol("a", "f");
    let b = symbol("b", "g");
    let first = Fragment {
        symbols: vec![a.clone()],
        edges: vec![call(&a, b.id.as_str()), call(&a, "external::h#function")],
        ..Fragment::default()
    };
    let second = Fragment {
        symbols: vec![b.clone()],
        ..Fragment::default()
    };
    let project = Project::merge([second.clone(), first, second]);
    assert_eq!(project.symbols.len(), 2);
    assert_eq!(project.edges[0].to, Target::Resolved(Node::Symbol(b.id)));
    assert_eq!(
        project.edges[1].to,
        Target::Path("external::h#function".to_owned())
    );
}
