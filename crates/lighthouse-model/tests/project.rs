use std::path::Path;

use lighthouse_model::{
    Edge, EdgeKind, File, Fragment, FunctionSummary, Module, Node, Position, Project, Resolution,
    Span, Symbol, SymbolId, SymbolKind, Target, Visibility,
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
        file: format!("{module}/{name}.go").into(),
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

fn summary(symbol: &Symbol, forwards_to: Option<Target>) -> FunctionSummary {
    FunctionSummary {
        symbol: symbol.id.clone(),
        max_nesting: 0,
        statements: 1,
        top_level: 1,
        params: 0,
        returns: 0,
        tokens: 0,
        flow: Vec::new(),
        clone_fingerprint: None,
        forwards_to,
    }
}

#[test]
fn kind_less_paths_resolve_only_when_they_name_one_symbol() {
    let f = symbol("p", "f");
    let mut field = symbol("p", "T");
    field.id = SymbolId::new("p", &["T"], "x", SymbolKind::Field);
    let mut method = symbol("p", "T");
    method.id = SymbolId::new("p", &["T"], "x", SymbolKind::Method);
    let fragment = Fragment {
        symbols: vec![f.clone(), field.clone(), method],
        edges: vec![call(&f, "p::f"), call(&f, "p::T::x"), call(&f, "p::none")],
        ..Fragment::default()
    };
    let project = Project::merge([fragment]);
    assert_eq!(project.edges[0].to, Target::Resolved(Node::Symbol(f.id)));
    assert_eq!(project.edges[1].to, Target::Path("p::T::x".to_owned()));
    assert_eq!(project.edges[2].to, Target::Path("p::none".to_owned()));
}

#[test]
fn module_paths_and_duplicate_edges_are_handled() {
    let a = symbol("a", "f");
    let import = Edge {
        kind: EdgeKind::Imports,
        from: Node::Module("a".to_owned()),
        to: Target::Path("b".to_owned()),
        resolution: Resolution::Syntactic,
    };
    let fragment = |module: &str| Fragment {
        modules: vec![Module {
            path: module.to_owned(),
            name: None,
            test_of: None,
        }],
        edges: vec![import.clone()],
        ..Fragment::default()
    };
    let project = Project::merge([
        fragment("a"),
        fragment("b"),
        Fragment {
            symbols: vec![a],
            ..Fragment::default()
        },
    ]);
    assert_eq!(project.edges.len(), 1);
    assert_eq!(
        project.edges[0].to,
        Target::Resolved(Node::Module("b".to_owned()))
    );
}

#[test]
fn project_indexes_answer_by_file_symbol_and_call_graph() {
    let a = symbol("m", "a");
    let b = symbol("m", "b");
    let c = symbol("n", "c");
    let test_file = File {
        path: b.file.clone(),
        lang: "go".to_owned(),
        hash: String::new(),
        generated: false,
        test: true,
    };
    let fragment = Fragment {
        files: vec![test_file],
        symbols: vec![a.clone(), b.clone(), c.clone()],
        edges: vec![
            call(&a, b.id.as_str()),
            call(&a, b.id.as_str()),
            call(&b, c.id.as_str()),
            call(&b, b.id.as_str()),
        ],
        functions: vec![summary(&a, Some(Target::Path(b.id.as_str().to_owned())))],
        ..Fragment::default()
    };
    let project = Project::merge([fragment]);
    assert_eq!(project.symbol(&b.id), Some(&b));
    assert_eq!(project.symbols_in(&b.file).collect::<Vec<_>>(), [&b]);
    assert_eq!(project.symbols_in(Path::new("none.go")).count(), 0);
    assert_eq!(project.callers(&b.id), std::slice::from_ref(&a.id));
    assert_eq!(project.callees(&b.id), std::slice::from_ref(&c.id));
    assert!(project.in_test(&b.id));
    assert!(!project.in_test(&a.id));
    assert!(project.file(&b.file).is_some_and(|f| f.test));
    assert_eq!(
        project.function(&a.id).and_then(|f| f.forwards_to.clone()),
        Some(Target::Resolved(Node::Symbol(b.id.clone())))
    );
    assert_eq!(b.id.module(), "m");
}

fn kinded(module: &str, owner: &str, name: &str, kind: SymbolKind, file: &str) -> Symbol {
    Symbol {
        id: SymbolId::new(module, &[owner], name, kind),
        kind,
        owner: None,
        file: file.into(),
        ..symbol(module, name)
    }
}

#[test]
fn duplicate_ids_keep_the_first_declaration_and_leave_a_notice() {
    let linux = kinded("p", "T", "run", SymbolKind::Method, "p/t_linux.go");
    let windows = kinded("p", "T", "run", SymbolKind::Method, "p/t_windows.go");
    let same_file_twice = Fragment {
        symbols: vec![linux.clone(), linux.clone()],
        ..Fragment::default()
    };
    assert!(Project::merge([same_file_twice]).notices().is_empty());

    let project = Project::merge([
        Fragment {
            symbols: vec![linux.clone()],
            ..Fragment::default()
        },
        Fragment {
            symbols: vec![windows],
            ..Fragment::default()
        },
    ]);
    assert_eq!(project.symbols, [linux]);
    assert_eq!(project.notices().len(), 1);
    let notice = &project.notices()[0];
    assert!(notice.contains("1 symbol id(s)"), "{notice}");
    assert!(
        notice.contains("p::T::run#method (p/t_linux.go, p/t_windows.go)"),
        "{notice}"
    );
}

#[test]
fn ambiguous_call_targets_prefer_callables_and_other_kinds_stay_unresolved_with_a_notice() {
    let caller = symbol("p", "caller");
    let field = kinded("p", "T", "x", SymbolKind::Field, "p/a.go");
    let method = kinded("p", "T", "x", SymbolKind::Method, "p/a.go");
    let var = kinded("p", "T", "y", SymbolKind::Var, "p/a.go");
    let konst = kinded("p", "T", "y", SymbolKind::Const, "p/a.go");
    let edge = |kind, to: &str| Edge {
        kind,
        from: Node::Symbol(caller.id.clone()),
        to: Target::Path(to.to_owned()),
        resolution: Resolution::Syntactic,
    };
    let project = Project::merge([Fragment {
        symbols: vec![caller.clone(), field, method.clone(), var, konst],
        edges: vec![
            edge(EdgeKind::Calls, "p::T::x"),
            edge(EdgeKind::References, "p::T::x"),
            edge(EdgeKind::Calls, "p::T::y"),
        ],
        ..Fragment::default()
    }]);
    let to: Vec<_> = project.edges.iter().map(|e| e.to.clone()).collect();
    assert_eq!(to[0], Target::Resolved(Node::Symbol(method.id)));
    assert_eq!(to[1], Target::Path("p::T::x".to_owned()));
    assert_eq!(to[2], Target::Path("p::T::y".to_owned()));
    assert_eq!(
        project.notices(),
        ["2 edge target(s) matched several symbols and stayed unresolved"]
    );
}

#[test]
fn references_index_lists_value_uses_not_calls() {
    let a = symbol("m", "a");
    let b = symbol("m", "b");
    let c = symbol("m", "c");
    let edge = |kind, from: &Symbol, to: &Symbol| Edge {
        kind,
        from: Node::Symbol(from.id.clone()),
        to: Target::Path(to.id.as_str().to_owned()),
        resolution: Resolution::Syntactic,
    };
    let project = Project::merge([Fragment {
        symbols: vec![a.clone(), b.clone(), c.clone()],
        edges: vec![
            edge(EdgeKind::Calls, &a, &c),
            edge(EdgeKind::References, &b, &c),
            edge(EdgeKind::References, &c, &c),
        ],
        ..Fragment::default()
    }]);
    assert_eq!(project.references(&c.id), std::slice::from_ref(&b.id));
    assert!(project.references(&a.id).is_empty());
    assert_eq!(project.callers(&c.id), std::slice::from_ref(&a.id));
}

#[test]
fn symbol_ids_built_elsewhere_must_follow_the_format() {
    let ok = SymbolId::parse("internal/store::Store::Get#method").unwrap();
    assert_eq!(ok.module(), "internal/store");
    assert!(SymbolId::parse("pkg::Run#test").is_some());
    for bad in [
        "nope",
        "pkg::Run",
        "pkg::Run#",
        "pkg::Run#thing",
        "pkg#function",
    ] {
        assert!(SymbolId::parse(bad).is_none(), "{bad}");
    }
}
