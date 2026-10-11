use std::path::Path;

use lighthouse_cache::{Digests, FileDigest};
use lighthouse_model::{
    Edge, EdgeKind, File, Fragment, FunctionSummary, Node, Position, Project, Resolution, Span,
    Symbol, SymbolId, SymbolKind, SymbolRole, Target, Visibility,
};

/// An edit of the callee.
type Change = fn(&mut World);

const A: &str = "a.rs";
const B: &str = "b.rs";

fn at(line: u32) -> Span {
    let start = Position { line, col: 1 };
    Span {
        start,
        end: Position { line, col: 9 },
    }
}

fn file(path: &str) -> File {
    File {
        path: path.into(),
        lang: "rust".to_owned(),
        hash: path.to_owned(),
        generated: false,
        test: false,
    }
}

fn symbol(name: &str, path: &str, line: u32) -> Symbol {
    Symbol {
        id: SymbolId::new("m", &[], name, SymbolKind::Function),
        kind: SymbolKind::Function,
        visibility: Visibility::Public,
        owner: None,
        file: path.into(),
        span: at(line),
        extent: None,
        doc: None,
        name: name.to_owned(),
        role: None,
        optional: false,
        type_ref: None,
    }
}

fn calls(from: &Symbol, to: &Symbol) -> Edge {
    Edge {
        kind: EdgeKind::Calls,
        from: Node::Symbol(from.id.clone()),
        to: Target::Path(to.id.as_str().to_owned()),
        resolution: Resolution::Semantic,
        site: None,
    }
}

fn summary(of: &Symbol) -> FunctionSummary {
    FunctionSummary {
        symbol: of.id.clone(),
        max_nesting: 0,
        statements: 1,
        top_level: 1,
        params: 0,
        returns: 0,
        tokens: 3,
        flow: Vec::new(),
        clone_fingerprint: None,
        forwards_to: None,
        signature: Default::default(),
        events: Vec::new(),
        manual_assertions: 0,
        implementation: false,
        constructs: false,
    }
}

/// The two files of a world: `a` calls `b`, and each file is a fragment.
struct World {
    a: Fragment,
    b: Fragment,
}

impl World {
    fn new() -> Self {
        let (a, b) = (symbol("a", A, 3), symbol("b", B, 3));
        Self {
            a: Fragment {
                files: vec![file(A)],
                edges: vec![calls(&a, &b)],
                functions: vec![summary(&a)],
                symbols: vec![a],
                ..Fragment::default()
            },
            b: Fragment {
                functions: vec![summary(&b)],
                files: vec![file(B)],
                symbols: vec![b],
                ..Fragment::default()
            },
        }
    }

    fn project(&self) -> Project {
        Project::merge([self.a.clone(), self.b.clone()])
    }

    fn digests(&self) -> Digests {
        Digests::of(&self.project()).unwrap()
    }

    /// The digests of a file after `change` was made to the callee `b`.
    fn changing(change: impl FnOnce(&mut World)) -> (FileDigest, FileDigest) {
        let before = World::new().digests();
        let mut world = World::new();
        change(&mut world);
        let after = world.digests();
        let of = |digests: &Digests| *digests.file(Path::new(A)).unwrap();
        (of(&before), of(&after))
    }
}

#[test]
fn a_neighbor_that_moves_leaves_the_neighbors_digest_and_changes_the_one_with_places() {
    let (before, after) = World::changing(|w| w.b.symbols[0].span = at(40));
    assert_eq!(before.slice, after.slice);
    assert_eq!(before.neighbors, after.neighbors);
    assert_ne!(before.neighbors_at, after.neighbors_at);
}

#[test]
fn every_field_a_neighbor_is_described_by_changes_the_neighbors_digest() {
    let changes: Vec<(&str, Change)> = vec![
        ("name", |w| w.b.symbols[0].name = "other".to_owned()),
        ("visibility", |w| {
            w.b.symbols[0].visibility = Visibility::Private
        }),
        ("kind", |w| w.b.symbols[0].kind = SymbolKind::Method),
        ("doc", |w| w.b.symbols[0].doc = Some("Docs.".to_owned())),
        ("role", |w| {
            w.b.symbols[0].role = Some(SymbolRole::TestHelper)
        }),
        ("lang", |w| w.b.files[0].lang = "go".to_owned()),
        ("test file", |w| w.b.files[0].test = true),
        ("generated file", |w| w.b.files[0].generated = true),
        ("implementation", |w| w.b.functions[0].implementation = true),
        ("owner", |w| {
            let owner = symbol("T", B, 1);
            w.b.symbols[0].owner = Some(owner.id.clone());
            w.b.symbols.push(owner);
        }),
        ("a second caller", |w| {
            let c = symbol("c", B, 9);
            let callee = w.b.symbols[0].clone();
            w.b.edges.push(calls(&c, &callee));
            w.b.symbols.push(c);
        }),
        ("a member", |w| {
            let mut member = symbol("m", B, 9);
            member.owner = Some(w.b.symbols[0].id.clone());
            member.kind = SymbolKind::Field;
            w.b.symbols.push(member);
        }),
    ];
    for (field, change) in changes {
        let (before, after) = World::changing(change);
        assert_ne!(before.neighbors, after.neighbors, "{field}");
        assert_ne!(before.neighbors_at, after.neighbors_at, "{field}");
    }
}

#[test]
fn a_caller_added_to_a_symbol_changes_the_neighbors_digest_of_its_file() {
    let (before, after) = World::changing(|w| {
        let c = symbol("c", "c.rs", 1);
        let a = w.a.symbols[0].clone();
        w.a.files.push(file("c.rs"));
        w.a.edges.push(calls(&c, &a));
        w.a.symbols.push(c);
    });
    assert_ne!(before.neighbors, after.neighbors);
}

#[test]
fn a_body_edit_of_a_neighbor_that_changes_nothing_it_is_described_by_changes_no_digest_of_the_file()
{
    let (before, after) = World::changing(|w| {
        w.b.functions[0].statements = 9;
        w.b.files[0].hash = "edited".to_owned();
    });
    assert_eq!(before, after);
}

#[test]
fn the_project_digest_is_stable_and_follows_any_change() {
    let world = World::new();
    assert_eq!(world.digests().project(), world.digests().project());
    let shuffled = Project::merge([world.b.clone(), world.a.clone()]);
    assert_eq!(
        Digests::of(&shuffled).unwrap().project(),
        world.digests().project()
    );
    let mut edited = World::new();
    edited.b.symbols[0].span = at(40);
    assert_ne!(edited.digests().project(), world.digests().project());
}

#[test]
fn the_slice_of_a_file_follows_its_own_content_only() {
    let mut edited = World::new();
    edited.b.symbols[0].span = at(40);
    let (old, new) = (World::new().digests(), edited.digests());
    let slice = |digests: &Digests, path: &str| digests.file(Path::new(path)).unwrap().slice;
    assert_eq!(slice(&old, A), slice(&new, A));
    assert_ne!(slice(&old, B), slice(&new, B));
}

#[test]
fn documents_digest_follows_their_content() {
    let document = |name: &str| lighthouse_model::Document {
        file: "d.yaml".into(),
        at: Position { line: 1, col: 1 },
        kind: "Decision".to_owned(),
        name: name.to_owned(),
        uid: None,
        labels: Default::default(),
    };
    let one = lighthouse_cache::documents(&[document("a")]).unwrap();
    assert_eq!(one, lighthouse_cache::documents(&[document("a")]).unwrap());
    assert_ne!(one, lighthouse_cache::documents(&[document("b")]).unwrap());
}
