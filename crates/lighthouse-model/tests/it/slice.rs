//! The model of a project as parts, put together to what `Project::merge`
//! makes of the same fragments.

use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use lighthouse_model::{
    Position, Span,
    ucm::{
        Edge, EdgeKind, Edges, Entry, File, Fragment, Incoming, Node, Outline, Project, Resolution,
        Resolved, Reverse, Sites, Slice, Symbol, SymbolId, SymbolKind, Table, Target, Visibility,
        collisions, declared,
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
        at(),
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
    Fragment {
        files: vec![File {
            path,
            lang: "go".to_owned(),
            hash: String::new(),
            generated: false,
            test: false,
            facts: Default::default(),
        }],
        symbols: vec![symbol],
        edges: edges.collect(),
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
    let outlines: Vec<Outline> = slices.iter().map(Outline::new).collect();
    let table = Table::new(&outlines);
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
        ..Fragment::default()
    }
}
