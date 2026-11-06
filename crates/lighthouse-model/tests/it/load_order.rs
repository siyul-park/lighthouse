//! What a file needs of its own declarations as it loads.

use lighthouse_model::ucm::Model;
use lighthouse_model::{
    Position, Span,
    ucm::{
        Edge, EdgeKind, File, Fragment, LoadNeed, LoadPoint, Node, ORDERED, Place, Project,
        Resolution, Symbol, SymbolId, SymbolKind, Target, Visibility, When,
    },
};

const FILE: &str = "m.py";

fn at(line: u32) -> Span {
    Span {
        start: Position { line, col: 1 },
        end: Position { line, col: 5 },
    }
}

fn id(owners: &[&str], name: &str) -> SymbolId {
    SymbolId::new("m", owners, name, SymbolKind::Function)
}

/// A declaration on `line`; `extent` says whether it has an extent, which the
/// body of a module has none of.
fn symbol(owners: &[&str], name: &str, line: u32, extent: bool) -> (Symbol, Place) {
    let mut symbol = Symbol::new(
        id(owners, name),
        SymbolKind::Function,
        Visibility::Private,
        FILE.into(),
        name,
    );
    symbol.owner = owners.split_last().map(|(owner, rest)| id(rest, owner));
    let place = Place {
        extent: extent.then(|| at(line)),
        ..Place::new(symbol.id.clone(), at(line))
    };
    (symbol, place)
}

fn edge(from: Option<SymbolId>, to: SymbolId, kind: EdgeKind, when: When, line: u32) -> Edge {
    Edge {
        kind,
        from: from.map_or_else(|| Node::Module("m".to_owned()), Node::Symbol),
        to: Target::Resolved(Node::Symbol(to)),
        resolution: Resolution::Semantic,
        site: Some(at(line)),
        when,
    }
}

fn project(ordered: bool, symbols: Vec<(Symbol, Place)>, edges: Vec<Edge>) -> Project {
    let (symbols, places) = symbols.into_iter().unzip();
    let mut file = File {
        path: FILE.into(),
        lang: "python".to_owned(),
        generated: false,
        test: false,
        facts: Default::default(),
    };
    if ordered {
        file.facts.insert(ORDERED.to_owned(), String::new());
    }
    Project::merge([Fragment {
        files: vec![file],
        symbols,
        places,
        edges,
        ..Fragment::default()
    }])
}

fn needs(project: &Project) -> Vec<(String, String)> {
    project
        .load_needs(FILE.as_ref())
        .into_iter()
        .map(|LoadNeed { user, binding }| {
            let user = match user {
                LoadPoint::Declaration(id) => project.symbol(&id).unwrap().name.clone(),
                LoadPoint::Site(site) => format!("line {}", site.start.line),
            };
            (user, project.symbol(&binding).unwrap().name.clone())
        })
        .collect()
}

#[test]
fn a_load_use_is_a_need_between_two_declarations() {
    let p = project(
        true,
        vec![symbol(&[], "a", 1, true), symbol(&[], "b", 3, true)],
        vec![edge(
            Some(id(&[], "b")),
            id(&[], "a"),
            EdgeKind::References,
            When::Load,
            3,
        )],
    );

    assert_eq!(needs(&p), [("b".to_owned(), "a".to_owned())]);
    assert!(p.load_breaks(FILE.as_ref()).is_empty());
}

#[test]
fn a_use_that_runs_when_called_is_no_need() {
    let p = project(
        true,
        vec![symbol(&[], "a", 1, true), symbol(&[], "b", 3, true)],
        vec![edge(
            Some(id(&[], "b")),
            id(&[], "a"),
            EdgeKind::References,
            When::Call,
            3,
        )],
    );

    assert!(needs(&p).is_empty());
}

#[test]
fn a_file_of_a_language_without_order_has_no_needs() {
    let p = project(
        false,
        vec![symbol(&[], "a", 1, true), symbol(&[], "b", 3, true)],
        vec![edge(
            Some(id(&[], "b")),
            id(&[], "a"),
            EdgeKind::References,
            When::Load,
            3,
        )],
    );

    assert!(needs(&p).is_empty());
}

#[test]
fn a_binding_below_the_code_that_needs_it_is_broken() {
    let p = project(
        true,
        vec![symbol(&[], "a", 1, true), symbol(&[], "b", 3, true)],
        vec![edge(
            Some(id(&[], "a")),
            id(&[], "b"),
            EdgeKind::References,
            When::Load,
            1,
        )],
    );

    let broken = p.load_breaks(FILE.as_ref());

    assert_eq!(broken.len(), 1);
    assert_eq!(broken[0].binding, id(&[], "b"));
}

#[test]
fn calling_a_function_at_load_needs_what_its_body_uses() {
    let p = project(
        true,
        vec![
            symbol(&[], "k", 1, true),
            symbol(&[], "f", 3, true),
            symbol(&[], "v", 5, true),
        ],
        vec![
            edge(
                Some(id(&[], "v")),
                id(&[], "f"),
                EdgeKind::Calls,
                When::Load,
                5,
            ),
            edge(
                Some(id(&[], "f")),
                id(&[], "k"),
                EdgeKind::References,
                When::Call,
                4,
            ),
        ],
    );

    assert_eq!(
        needs(&p),
        [
            ("v".to_owned(), "f".to_owned()),
            ("v".to_owned(), "k".to_owned())
        ]
    );
}

#[test]
fn a_member_of_a_class_needs_its_sibling_and_the_class_what_is_outside() {
    let class = id(&[], "A");
    let mut x = symbol(&["A"], "x", 3, true);
    x.0.owner = Some(class.clone());
    let mut y = symbol(&["A"], "y", 4, true);
    y.0.owner = Some(class.clone());
    let p = project(
        true,
        vec![symbol(&[], "k", 1, true), symbol(&[], "A", 2, true), x, y],
        vec![
            edge(
                Some(id(&["A"], "y")),
                id(&["A"], "x"),
                EdgeKind::References,
                When::Load,
                4,
            ),
            edge(
                Some(id(&["A"], "x")),
                id(&[], "k"),
                EdgeKind::References,
                When::Load,
                3,
            ),
        ],
    );

    assert_eq!(
        needs(&p),
        [
            ("A".to_owned(), "k".to_owned()),
            ("y".to_owned(), "x".to_owned())
        ]
    );
}

#[test]
fn top_level_code_needs_a_binding_where_the_project_keeps_its_use() {
    let p = project(
        true,
        vec![symbol(&[], "a", 1, true)],
        vec![
            edge(None, id(&[], "a"), EdgeKind::Calls, When::Load, 7),
            edge(None, id(&[], "a"), EdgeKind::Calls, When::Load, 9),
        ],
    );

    assert_eq!(needs(&p), [("line 7".to_owned(), "a".to_owned())]);
}

#[test]
fn the_body_of_a_module_has_no_extent_and_counts_where_it_uses() {
    let p = project(
        true,
        vec![
            symbol(&[], "<module>", 1, false),
            symbol(&[], "a", 20, true),
        ],
        vec![edge(
            Some(id(&[], "<module>")),
            id(&[], "a"),
            EdgeKind::Calls,
            When::Load,
            8,
        )],
    );

    assert_eq!(needs(&p), [("line 8".to_owned(), "a".to_owned())]);
    assert_eq!(p.load_breaks(FILE.as_ref()).len(), 1);
}

#[test]
fn key_names_the_declaration_that_uses_or_none_for_top_level_code() {
    let declared = LoadNeed {
        user: LoadPoint::Declaration(id(&[], "b")),
        binding: id(&[], "a"),
    };
    let top = LoadNeed {
        user: LoadPoint::Site(at(9)),
        binding: id(&[], "a"),
    };

    assert_eq!(declared.key(), (Some(&id(&[], "b")), &id(&[], "a")));
    assert_eq!(top.key(), (None, &id(&[], "a")));
}

#[test]
fn load_place_is_where_the_code_starts() {
    let p = project(true, vec![symbol(&[], "a", 4, true)], vec![]);

    assert_eq!(
        p.load_place(&LoadPoint::Declaration(id(&[], "a"))),
        Some(Position { line: 4, col: 1 })
    );
    assert_eq!(
        p.load_place(&LoadPoint::Site(at(7))),
        Some(Position { line: 7, col: 1 })
    );
    assert_eq!(p.load_place(&LoadPoint::Declaration(id(&[], "b"))), None);
}

#[test]
fn is_ordered_follows_the_fact_the_host_states() {
    let ordered = project(true, vec![], vec![]);
    let free = project(false, vec![], vec![]);

    assert!(ordered.file(FILE.as_ref()).unwrap().is_ordered());
    assert!(!free.file(FILE.as_ref()).unwrap().is_ordered());
}

#[test]
fn is_call_is_the_when_a_use_has_unless_it_says_load() {
    assert!(When::default().is_call());
    assert!(!When::Load.is_call());
}

#[test]
fn load_needs() {
    let edges = vec![edge(
        Some(id(&[], "b")),
        id(&[], "a"),
        EdgeKind::References,
        When::Load,
        3,
    )];
    let symbols = vec![symbol(&[], "a", 1, true), symbol(&[], "b", 3, true)];
    let ordered = project(true, symbols.clone(), edges.clone());
    let free = project(false, symbols, edges);

    let found = lighthouse_model::ucm::load_needs(&ordered, FILE.as_ref(), ordered.edges());
    let unordered = lighthouse_model::ucm::load_needs(&free, FILE.as_ref(), free.edges());

    assert_eq!(found, ordered.load_needs(FILE.as_ref()));
    assert_eq!(found.len(), 1);
    assert!(unordered.is_empty());
}
