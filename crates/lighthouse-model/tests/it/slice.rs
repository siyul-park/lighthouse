//! The model of a project as parts, put together to what `Project::merge`
//! makes of the same fragments.

use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use lighthouse_model::{
    Position, Span,
    ucm::{
        Declarations, Edge, EdgeKind, Edges, Entry, File, Fragment, Incoming, Module, Node, Place,
        Project, Resolution, Resolved, Reverse, Sites, Slice, Symbol, SymbolId, SymbolKind, Table,
        Target, Visibility, collisions, declared,
    },
};

fn at() -> Span {
    let p = Position { line: 1, col: 1 };
    Span { start: p, end: p }
}

fn fragment(module: &str, name: &str, callee: Option<&str>) -> Fragment {
    let id = SymbolId::new(module, &[], name, SymbolKind::Function);
    let path = PathBuf::from(format!("{module}/{name}.go"));
    let symbol = Symbol::new(
        id.clone(),
        SymbolKind::Function,
        Visibility::Public,
        path.clone(),
        name,
    );
    let edges = callee.into_iter().map(|to| Edge {
        kind: EdgeKind::Calls,
        from: Node::Symbol(id.clone()),
        to: Target::Path(to.to_owned()),
        resolution: Resolution::Syntactic,
        site: None,
        when: Default::default(),
    });
    let edges: Vec<Edge> = edges.collect();
    Fragment {
        files: vec![File {
            path,
            lang: "go".to_owned(),
            generated: false,
            test: false,
            facts: Default::default(),
        }],
        symbols: vec![symbol],
        places: vec![Place::new(id.clone(), at())],
        edges,
        ..Fragment::default()
    }
}

#[test]
fn assemble_the_slices_resolved_against_the_table_as_merge_does() {
    let b = SymbolId::new("b", &[], "g", SymbolKind::Function);
    let fragments = [
        fragment("a", "f", Some(b.as_str())),
        fragment("b", "g", None),
    ];
    assert!(collisions(fragments.iter().flat_map(declared)).is_empty());
    let none = BTreeSet::new();
    let slices: Vec<Slice> = fragments
        .iter()
        .map(|part| {
            let path = part.files[0].path.clone();
            Slice::new(path, vec![part.clone()], &none)
        })
        .collect();
    let declarations: Vec<Declarations> = slices.iter().map(Declarations::new).collect();
    let table = Table::new(&declarations);
    let entry: &Entry = table.entry(&b).unwrap();
    assert_eq!(entry.name, "g");
    let resolved: Vec<Resolved> = slices.iter().map(|s| Resolved::new(s, &table)).collect();
    let edges = Edges::new(&table, &resolved);
    let reverse: &Reverse = &edges.reverse;
    let incoming = Incoming::new(&resolved[1], &table, reverse);
    assert_eq!(incoming.by_symbol[&b].callers.len(), 1);
    let merged = Project::merge(fragments.clone());
    let sites = Arc::new(Sites::new(&table, &resolved));
    assert_eq!(Project::assemble(&resolved, (&edges, &sites)), merged);
    assert_eq!(Table::of(&fragment_of(&merged)), table);
}

fn fragment_of(project: &Project) -> Fragment {
    Fragment {
        files: project.files.clone(),
        modules: project.modules.clone(),
        symbols: project.symbols.clone(),
        places: project.places.clone(),
        ..Fragment::default()
    }
}

#[test]
fn table_modules() {
    let module = |path: &str| Module {
        path: path.to_owned(),
        name: None,
        test_of: None,
        language: None,
        example: false,
    };
    let table = Table::of(&Fragment {
        modules: vec![module("b"), module("a")],
        ..Fragment::default()
    });

    let paths: Vec<&str> = table.modules().map(|module| module.path.as_str()).collect();

    assert_eq!(paths, ["a", "b"]);
}
