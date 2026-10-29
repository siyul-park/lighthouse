use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use lighthouse_model::{
    Document, Position, Span,
    ucm::{
        Comment, Edge, EdgeKind, File, Fragment, FunctionSummary, Module, Node, Project,
        Resolution, Symbol, SymbolId, SymbolKind, Target, TestCase, TestStyle, Visibility,
    },
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
        extent: None,
        doc: None,
        name: name.to_owned(),
        role: None,
        optional: false,
        type_ref: None,
    }
}

fn call(from: &Symbol, to: &str) -> Edge {
    Edge {
        kind: EdgeKind::Calls,
        from: Node::Symbol(from.id.clone()),
        to: Target::Path(to.to_owned()),
        resolution: Resolution::Syntactic,
        site: None,
    }
}

#[test]
fn file_analysis_language_defaults_to_host_and_honors_embedded_override() {
    let mut file = File {
        path: PathBuf::from("settings.yaml"),
        lang: "yaml".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
        facts: BTreeMap::new(),
    };
    assert_eq!(file.analysis_language(), "yaml");

    file.facts
        .insert("analysisLanguage".to_owned(), "cel".to_owned());
    assert_eq!(file.analysis_language(), "cel");
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
fn symbol_kind_as_str() {
    for kind in [
        SymbolKind::Function,
        SymbolKind::Test,
        SymbolKind::Interface,
    ] {
        let id = SymbolId::new("m", &[], "x", kind);
        assert!(id.as_str().ends_with(&format!("#{}", kind.as_str())));
    }
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
        forwards_to,
        signature: Default::default(),
        events: Vec::new(),
        manual_assertions: 0,
        implementation: false,
        constructs: false,
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
        site: None,
    };
    let fragment = |module: &str| Fragment {
        modules: vec![Module {
            path: module.to_owned(),
            name: None,
            test_of: None,
            language: None,
            example: false,
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
fn a_module_edge_remembers_the_file_of_the_first_fragment_that_has_it() {
    let import = |to: &str| Edge {
        kind: EdgeKind::Imports,
        from: Node::Module("a".to_owned()),
        to: Target::Path(to.to_owned()),
        resolution: Resolution::Syntactic,
        site: None,
    };
    let fragment = |path: &str, edges: Vec<Edge>| Fragment {
        files: vec![File {
            path: path.into(),
            lang: "go".to_owned(),
            hash: String::new(),
            generated: false,
            test: false,
            facts: Default::default(),
        }],
        edges,
        ..Fragment::default()
    };
    let project = Project::merge([
        fragment("a/one.go", vec![import("b")]),
        fragment("a/two.go", vec![import("b"), import("c")]),
        Fragment {
            edges: vec![import("d")],
            ..Fragment::default()
        },
    ]);
    let origins: Vec<Option<&Path>> = (0..project.edges.len())
        .map(|at| project.origin(at))
        .collect();
    assert_eq!(
        origins,
        [
            None,
            Some(Path::new("a/one.go")),
            Some(Path::new("a/two.go")),
        ]
    );
}

#[test]
fn an_import_at_several_sites_is_one_edge_with_the_site_of_the_first_file_in_path_order() {
    let import = |line: u32| Edge {
        kind: EdgeKind::Imports,
        from: Node::Module("a".to_owned()),
        to: Target::Path("b".to_owned()),
        resolution: Resolution::Syntactic,
        site: Some(Span {
            start: Position { line, col: 1 },
            end: Position { line, col: 9 },
        }),
    };
    let fragment = |path: &str, edges: Vec<Edge>| Fragment {
        files: vec![File {
            path: path.into(),
            lang: "go".to_owned(),
            hash: String::new(),
            generated: false,
            test: false,
            facts: Default::default(),
        }],
        edges,
        ..Fragment::default()
    };
    // The providers answered out of path order.
    let project = Project::merge([
        fragment("a/two.go", vec![import(3)]),
        fragment("a/one.go", vec![import(5), import(7)]),
    ]);
    assert_eq!(project.edges.len(), 1);
    assert_eq!(project.edges[0].site.map(|s| s.start.line), Some(5));
    assert_eq!(project.origin(0), Some(Path::new("a/one.go")));
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
        facts: Default::default(),
    };
    let fragment = Fragment {
        files: vec![test_file],
        modules: vec![Module {
            path: "m".to_owned(),
            name: None,
            test_of: None,
            language: None,
            example: false,
        }],
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
    assert!(project.module("m").is_some());
    assert!(project.module("n").is_none());
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
        site: None,
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
        site: None,
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

#[test]
fn heuristic_edges_are_possible_uses_not_calls() {
    let a = symbol("a", "f");
    let b = symbol("a", "g");
    let guess = |kind| Edge {
        kind,
        from: Node::Symbol(a.id.clone()),
        to: Target::Path(b.id.as_str().to_owned()),
        resolution: Resolution::Heuristic,
        site: None,
    };
    let project = Project::merge([Fragment {
        symbols: vec![a.clone(), b.clone()],
        edges: vec![guess(EdgeKind::Calls), guess(EdgeKind::References)],
        ..Fragment::default()
    }]);
    assert!(project.callers(&b.id).is_empty());
    assert!(project.callees(&a.id).is_empty());
    assert_eq!(project.references(&b.id), [a.id]);
}

fn comment(file: &str, line: u32, text: &str) -> Comment {
    let p = Position { line, col: 1 };
    Comment {
        file: file.into(),
        span: Span { start: p, end: p },
        text: text.to_owned(),
        attached_to: None,
    }
}

#[test]
fn comments_are_sorted_and_found_by_file() {
    let project = Project::merge([
        Fragment {
            comments: vec![
                comment("b.go", 3, "// late"),
                comment("b.go", 1, "// early"),
            ],
            ..Fragment::default()
        },
        Fragment {
            comments: vec![comment("a.go", 2, "// other")],
            ..Fragment::default()
        },
    ]);
    let texts = |path: &str| -> Vec<&str> {
        project
            .comments_in(Path::new(path))
            .iter()
            .map(|c| c.text.as_str())
            .collect()
    };
    assert_eq!(texts("b.go"), ["// early", "// late"]);
    assert_eq!(texts("a.go"), ["// other"]);
    assert!(texts("c.go").is_empty());
}

#[test]
fn indexes_answer_uses_members_and_tests_by_module() {
    let ty = Symbol {
        kind: SymbolKind::Type,
        id: SymbolId::new("m", &[], "T", SymbolKind::Type),
        name: "T".to_owned(),
        ..symbol("m", "T")
    };
    let method = Symbol {
        kind: SymbolKind::Method,
        id: SymbolId::new("m", &["T"], "run", SymbolKind::Method),
        owner: Some(ty.id.clone()),
        name: "run".to_owned(),
        ..symbol("m", "run")
    };
    let user = symbol("m", "user");
    let test = Symbol {
        kind: SymbolKind::Test,
        id: SymbolId::new("m[test]", &[], "TestT", SymbolKind::Test),
        name: "TestT".to_owned(),
        ..symbol("m", "TestT")
    };
    let case = TestCase {
        symbol: test.id.clone(),
        nesting: 0,
        style: TestStyle::Scenario,
        targets: vec![Target::Path(ty.id.as_str().to_owned())],
    };
    let project = Project::merge([Fragment {
        symbols: vec![ty.clone(), method.clone(), user.clone(), test.clone()],
        edges: vec![
            call(&user, method.id.as_str()),
            call(&user, user.id.as_str()),
        ],
        tests: vec![case.clone()],
        ..Fragment::default()
    }]);
    assert_eq!(project.members(&ty.id), std::slice::from_ref(&method.id));
    assert_eq!(
        project.uses(&user.id),
        std::slice::from_ref(&method.id),
        "no self use"
    );
    assert_eq!(project.test(&test.id).map(|t| t.nesting), Some(0));
    assert_eq!(project.tests_in("m[test]").len(), 1);
    assert!(project.tests_in("m").is_empty(), "a prefix is not a module");
}

fn typed(module: &str, name: &str, kind: SymbolKind, owner: Option<&Symbol>) -> Symbol {
    let owners: Vec<&str> = owner.iter().map(|o| o.name.as_str()).collect();
    Symbol {
        id: SymbolId::new(module, &owners, name, kind),
        kind,
        owner: owner.map(|o| o.id.clone()),
        ..symbol(module, name)
    }
}

fn implements(ty: &Symbol, interface: &Symbol) -> Edge {
    Edge {
        kind: EdgeKind::Implements,
        from: Node::Symbol(ty.id.clone()),
        to: Target::Resolved(Node::Symbol(interface.id.clone())),
        resolution: Resolution::Semantic,
        site: None,
    }
}

#[test]
fn project_edges_from_lists_the_edges_that_start_at_a_symbol() {
    let a = symbol("m", "a");
    let b = symbol("m", "b");
    let project = Project::merge([Fragment {
        files: vec![file_of(&a), file_of(&b)],
        symbols: vec![a.clone(), b.clone()],
        edges: vec![call(&a, "m::b#function"), call(&b, "m::a#function")],
        ..Fragment::default()
    }]);

    let from_a: Vec<_> = project.edges_from(&a.id).collect();

    assert_eq!(from_a.len(), 1);
    assert_eq!(from_a[0].from, Node::Symbol(a.id.clone()));
    assert_eq!(
        project
            .edges_from(&SymbolId::new("m", &[], "none", SymbolKind::Function))
            .count(),
        0
    );
}

#[test]
fn project_implementers_follow_the_interfaces_of_a_type() {
    let store = typed("m", "Store", SymbolKind::Type, None);
    let reader = typed("m", "Reader", SymbolKind::Interface, None);
    let read = typed("m", "Read", SymbolKind::Method, Some(&reader));
    let project = Project::merge([Fragment {
        files: vec![file_of(&store)],
        symbols: vec![store.clone(), reader.clone(), read],
        edges: vec![implements(&store, &reader)],
        ..Fragment::default()
    }]);

    assert_eq!(
        project.implementers(&reader.id),
        std::slice::from_ref(&store.id)
    );
    assert!(project.implementers(&store.id).is_empty());
}

fn file_of(symbol: &Symbol) -> File {
    File {
        path: symbol.file.clone(),
        lang: "go".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
        facts: Default::default(),
    }
}

#[test]
fn project_with_documents_keeps_them_in_file_order_and_project_documents_lists_them() {
    let document = |file: &str, line: u32, name: &str| Document {
        file: file.into(),
        at: Position { line, col: 9 },
        kind: "Decision".to_owned(),
        name: name.to_owned(),
        uid: None,
        labels: Default::default(),
    };
    let project = Project::merge([Fragment::default()]);
    assert!(project.documents().is_empty());

    let project = project.with_documents(vec![
        document("b.yaml", 2, "p/b"),
        document("a.yaml", 7, "p/z"),
        document("a.yaml", 3, "p/a"),
    ]);

    let names: Vec<_> = project
        .documents()
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(names, ["p/a", "p/z", "p/b"]);
}

fn host(facts: &[(&str, &str)]) -> Fragment {
    Fragment {
        files: vec![File {
            path: PathBuf::from("README.md"),
            lang: "markdown".to_owned(),
            hash: String::new(),
            generated: false,
            test: false,
            facts: facts
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        }],
        ..Fragment::default()
    }
}

#[test]
fn what_the_readers_of_regions_state_of_a_host_file_wins_over_what_the_host_states() {
    let project = Project::merge([
        host(&[("edition", "2015"), ("keep", "host")]),
        host(&[("edition", "2021")]),
    ]);

    let facts = &project.file(Path::new("README.md")).unwrap().facts;
    assert_eq!(facts["edition"], "2021");
    assert_eq!(facts["keep"], "host");
}

#[test]
fn a_module_a_region_shares_with_the_project_is_the_projects() {
    let module = |example: bool, language: Option<&str>| Fragment {
        modules: vec![Module {
            path: "pkg".to_owned(),
            name: None,
            test_of: None,
            language: language.map(str::to_owned),
            example,
        }],
        ..Fragment::default()
    };

    let project = Project::merge([module(true, Some("go")), module(false, None)]);

    let module = project.module("pkg").unwrap();
    assert!(!module.example);
    assert_eq!(module.language, None);
}

#[test]
fn code_in_a_host_file_of_examples_is_example_code_in_any_module() {
    let f = symbol("pkg", "f");
    let g = symbol("pkg", "g");
    let mut shown = symbol("pkg", "shown");
    shown.file = PathBuf::from("README.md");
    let fence_call = Edge {
        kind: EdgeKind::Calls,
        from: Node::Symbol(shown.id.clone()),
        to: Target::Path(f.id.as_str().to_owned()),
        resolution: Resolution::Semantic,
        site: None,
    };
    let own_call = Edge {
        kind: EdgeKind::Calls,
        from: Node::Symbol(g.id.clone()),
        to: Target::Path(f.id.as_str().to_owned()),
        resolution: Resolution::Semantic,
        site: None,
    };
    let project = Project::merge([
        host(&[("regions", "go:example")]),
        Fragment {
            symbols: vec![f.clone(), g.clone(), shown.clone()],
            edges: vec![fence_call, own_call],
            files: vec![file_of(&f)],
            ..Fragment::default()
        },
    ]);

    assert!(project.in_example(&shown.id));
    assert!(!project.in_example(&f.id));
    assert_eq!(project.callers(&f.id), std::slice::from_ref(&g.id));
}

#[test]
fn a_syntactic_edge_to_a_bare_name_reaches_the_one_symbol_of_the_project_so_named() {
    let f = symbol("pkg", "Parse");
    let twin_a = symbol("a", "Twin");
    let twin_b = symbol("b", "Twin");
    let user = symbol("pkg", "user");
    let to = |name: &str| Edge {
        kind: EdgeKind::Calls,
        from: Node::Symbol(user.id.clone()),
        to: Target::Path(name.to_owned()),
        resolution: Resolution::Syntactic,
        site: None,
    };
    let project = Project::merge([Fragment {
        symbols: vec![f.clone(), twin_a, twin_b, user.clone()],
        edges: vec![to("Parse"), to("Twin"), to("Missing")],
        ..Fragment::default()
    }]);

    let targets: Vec<&Target> = project.edges.iter().map(|edge| &edge.to).collect();
    assert!(targets.contains(&&Target::Resolved(Node::Symbol(f.id))));
    assert!(targets.contains(&&Target::Path("Twin".to_owned())));
    assert!(targets.contains(&&Target::Path("Missing".to_owned())));
}

#[test]
fn file_is_example_when_every_region_of_it_is_example_code() {
    let file = |regions: Option<&str>| File {
        path: PathBuf::from("README.md"),
        lang: "markdown".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
        facts: regions
            .map(|r| BTreeMap::from([("regions".to_owned(), r.to_owned())]))
            .unwrap_or_default(),
    };

    assert!(file(Some("go:example,yaml:example")).is_example());
    assert!(!file(Some("go:example,yaml:code")).is_example());
    assert!(
        !file(None).is_example(),
        "a file with no regions is not a host"
    );
}
