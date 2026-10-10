//! What the neighbors digest of the result cache must see: every field of a
//! node, and every fact declared to reach no further than the neighbors, when
//! it changes because of an edit in another file.

use std::path::Path;

use lighthouse_cache::Digests;
use lighthouse_checks::probe;
use lighthouse_model::{
    Applicability, EdgeKind, File, Project, Symbol, SymbolId, SymbolKind, SymbolRole, Visibility,
};
use lighthouse_plugin::{Ctx, Memo, NoKeys, Notices, Workspace};
use serde_json::Value;

use crate::support::World;

const F1: &str = "m/a.ucm";
const F2: &str = "m/b.ucm";
const F3: &str = "m/c.ucm";

/// The fields of a node that say where a symbol is in its file; the neighbors
/// digest leaves them out and `neighbors_at` has them.
const POSITIONS: [&str; 4] = ["line", "col", "pos", "end_line"];

fn find<'w>(world: &'w mut World, name: &str) -> &'w mut Symbol {
    world.symbols.iter_mut().find(|s| s.name == name).unwrap()
}

fn symbol(project: &Project, name: &str) -> Symbol {
    project
        .symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap()
        .clone()
}

fn ctx_run<R>(project: &Project, file: Option<&File>, run: impl FnOnce(&Ctx) -> R) -> R {
    let ws = Workspace::new(".");
    let facts = Default::default();
    let memo = Memo::default();
    let notices = Notices::default();
    let ctx = Ctx {
        ws: &ws,
        project,
        file: file.map(|f| (f, "")),
        facts: &facts,
        keys: &NoKeys,
        trusted: false,
        memo: &memo,
        notices: &notices,
        applies: Applicability::default(),
    };
    run(&ctx)
}

fn digests(project: &Project, file: &str) -> (lighthouse_cache::FileDigest, Digests) {
    let all = Digests::of(project).unwrap();
    (*all.file(Path::new(file)).unwrap(), all)
}

/// A callee `b` in `m/b.ucm` and its caller `a` in `m/a.ucm`.
fn pair() -> World {
    let mut w = World::default();
    let a = w.func("m", "a", F1);
    let b = w.func("m", "b", F2);
    w.edge(EdgeKind::Calls, &a, &b);
    w
}

type Edit = fn(&mut World);

/// How each field of a node (but the positions) is changed in the callee.
const NODE_FIELDS: [(&str, Edit); 16] = [
    ("id", |w| rename(w, "b", "n", "b")),
    ("name", |w| rename(w, "b", "m", "renamed")),
    ("kind", |w| find(w, "b").kind = SymbolKind::Method),
    ("visibility", |w| {
        find(w, "b").visibility = Visibility::Private
    }),
    ("owner", |w| {
        let owner = w.symbol("m", "T", SymbolKind::Type, F2);
        find(w, "b").owner = Some(owner.id);
    }),
    ("owner_kind", |w| {
        let owner = w.symbol("m", "T", SymbolKind::Type, F2);
        find(w, "b").owner = Some(owner.id);
        find(w, "T").kind = SymbolKind::Interface;
    }),
    ("file", |w| {
        w.files.entry(F3.to_owned()).or_insert(false);
        find(w, "b").file = F3.into();
    }),
    ("module", |w| rename(w, "b", "n", "b")),
    ("test_role", |w| {
        find(w, "b").role = Some(SymbolRole::Fixture)
    }),
    ("documented", |w| {
        find(w, "b").doc = Some("Docs.".to_owned())
    }),
    ("test", |w| {
        w.files.remove(F2);
        w.files.insert("m/b_test.ucm".to_owned(), false);
        find(w, "b").file = "m/b_test.ucm".into();
    }),
    ("file_test", |w| {
        w.files.remove(F2);
        w.files.insert("m/b_test.ucm".to_owned(), false);
        find(w, "b").file = "m/b_test.ucm".into();
    }),
    ("generated", |w| {
        w.files.insert(F2.to_owned(), true);
    }),
    ("owner_key", |w| {
        find(w, "b").kind = SymbolKind::Method;
        rename(w, "b", "m::T", "b");
    }),
    ("declaration", |w| find(w, "b").kind = SymbolKind::Field),
    ("implementation", |w| {
        w.summaries.iter_mut().for_each(|s| s.implementation = true);
    }),
];

/// Gives the symbol `name` the module `module` and the name `to`, and points
/// the edges at it.
fn rename(w: &mut World, name: &str, module: &str, to: &str) {
    let old = find(w, name).id.clone();
    let new = SymbolId::new(module, &[], to, find(w, name).kind);
    for s in &mut w.symbols {
        if s.id == old {
            s.id = new.clone();
            s.name = to.to_owned();
        }
    }
    for f in &mut w.summaries {
        if f.symbol == old {
            f.symbol = new.clone();
        }
    }
    for e in &mut w.edges {
        if matches!(&e.to, lighthouse_model::Target::Path(p) if p == old.as_str()) {
            e.to = lighthouse_model::Target::Path(new.as_str().to_owned());
        }
    }
}

#[test]
fn the_edits_cover_every_field_of_a_node() {
    let w = pair();
    let project = w.project();
    let mut fields = probe::node_fields(&project, &symbol(&project, "b"));
    fields.retain(|field| !POSITIONS.contains(&field.as_str()));
    fields.sort();
    let mut covered: Vec<&str> = NODE_FIELDS.iter().map(|(field, _)| *field).collect();
    covered.sort_unstable();
    assert_eq!(
        fields, covered,
        "a field of a node needs an edit here, and the neighbors digest must see it"
    );
}

#[test]
fn every_field_of_a_node_changes_the_neighbors_digest_of_its_caller() {
    let before = digests(&pair().project(), F1).0;
    for (field, edit) in NODE_FIELDS {
        let mut w = pair();
        edit(&mut w);
        let after = digests(&w.project(), F1).0;
        assert_ne!(before.neighbors, after.neighbors, "{field}");
    }
}

/// An edit in another file that changes a fact of a subject, built so that
/// nothing in the subject's own file changes.
struct Case {
    fact: &'static str,
    /// The file the subject is in.
    file: &'static str,
    /// The symbol the fact is read of; the file for a file fact.
    subject: &'static str,
    build: fn() -> World,
    edit: Edit,
    /// Whether the file's own slice is untouched by the edit; the owner of a
    /// symbol is part of the file's slice, so that edit is not.
    elsewhere: bool,
}

fn read(case: &Case, project: &Project) -> Value {
    let file = project.file(Path::new(case.file)).unwrap();
    ctx_run(project, Some(file), |ctx| match case.fact {
        "private_reach" => probe::file_fact(ctx, file, "", case.fact),
        _ => probe::symbol_fact(ctx, &symbol(project, case.subject), case.fact),
    })
    .unwrap()
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            fact: "envy",
            file: F1,
            subject: "total",
            build: || {
                let mut w = World::default();
                let store = w.symbol("m", "Store", SymbolKind::Type, F2);
                let total = w.func("m", "total", F1);
                for name in ["items", "len"] {
                    let field = w.member(&store, name, SymbolKind::Field, F2);
                    w.edge(EdgeKind::References, &total, &field);
                }
                w
            },
            // A variable elsewhere refers to it: it is wiring.
            edit: |w| {
                let total = symbol(&w.project(), "total");
                let table = w.symbol("m", "table", SymbolKind::Var, F3);
                w.edge(EdgeKind::References, &table, &total);
            },
            elsewhere: true,
        },
        Case {
            fact: "receiver_affinity",
            file: F1,
            subject: "helper",
            build: || {
                let mut w = World::default();
                let store = w.symbol("m", "Store", SymbolKind::Type, F2);
                let run = w.member(&store, "run", SymbolKind::Method, F2);
                let helper = w.func("m", "helper", F1);
                w.private(&helper);
                w.edge(EdgeKind::Calls, &run, &helper);
                w
            },
            // A free function elsewhere calls it too: no single owner.
            edit: |w| {
                let helper = symbol(&w.project(), "helper");
                let other = w.func("m", "other", F3);
                w.edge(EdgeKind::Calls, &other, &helper);
            },
            elsewhere: true,
        },
        Case {
            fact: "owner_home",
            file: F2,
            subject: "run",
            build: || {
                let mut w = World::default();
                let store = w.symbol("m", "Store", SymbolKind::Type, F1);
                w.member(&store, "run", SymbolKind::Method, F2);
                w
            },
            // The type moves to another file.
            edit: |w| {
                w.files.entry(F3.to_owned()).or_insert(false);
                find(w, "Store").file = F3.into();
            },
            elsewhere: true,
        },
        Case {
            fact: "data_only",
            file: F1,
            subject: "Store",
            build: || {
                let mut w = World::default();
                w.symbol("m", "Store", SymbolKind::Type, F1);
                w
            },
            // Another file gives the type a method.
            edit: |w| {
                let store = symbol(&w.project(), "Store");
                w.member(&store, "run", SymbolKind::Method, F2);
            },
            elsewhere: true,
        },
        Case {
            fact: "private_reach",
            file: F1,
            subject: "a",
            build: || {
                let mut w = World::default();
                let a = w.func("m", "a", F1);
                let hidden = w.func("m", "hidden", F2);
                w.private(&hidden);
                w.edge(EdgeKind::Calls, &a, &hidden);
                w
            },
            // The callee in the other file is no longer private.
            edit: |w| find(w, "hidden").visibility = Visibility::Public,
            elsewhere: true,
        },
        Case {
            fact: "helper_user",
            file: F1,
            subject: "helper",
            build: || {
                let mut w = World::default();
                let helper = w.func("m", "helper", F1);
                let owner = w.symbol("m", "Owner", SymbolKind::Type, F2);
                let user = w.member(&owner, "user", SymbolKind::Method, F1);
                w.edge(EdgeKind::Calls, &user, &helper);
                w
            },
            // The type of the user, in another file, becomes an interface.
            edit: |w| find(w, "Owner").kind = SymbolKind::Interface,
            elsewhere: false,
        },
    ]
}

#[test]
fn a_fact_of_the_neighbors_that_changes_with_an_edit_elsewhere_changes_the_neighbors_digest() {
    let declared: Vec<_> = lighthouse_checks::reach::FACTS
        .iter()
        .filter(|(_, reach)| *reach == lighthouse_model::Reach::Neighbors)
        .map(|(fact, _)| *fact)
        .collect();
    let cases = cases();
    for case in &cases {
        let mut world = (case.build)();
        let before_project = world.project();
        let (before, _) = digests(&before_project, case.file);
        let value_before = read(case, &before_project);

        (case.edit)(&mut world);
        let after_project = world.project();
        let (after, _) = digests(&after_project, case.file);
        let value_after = read(case, &after_project);

        let fact = case.fact;
        assert_ne!(
            value_before, value_after,
            "{fact}: the edit changes nothing"
        );
        if case.elsewhere {
            assert_eq!(
                before.slice, after.slice,
                "{fact}: the edit is not elsewhere"
            );
        }
        assert_ne!(before.neighbors, after.neighbors, "{fact}: a stale result");
    }
    let tested: Vec<_> = cases.iter().map(|case| case.fact).collect();
    for fact in [
        "envy",
        "receiver_affinity",
        "owner_home",
        "data_only",
        "private_reach",
        "helper_user",
    ] {
        assert!(declared.contains(&fact) && tested.contains(&fact), "{fact}");
    }
}
