//! The edge sets that can be rendered over a list of declarations.

use super::{Decl, Edge, Kind, Relation};

/// Every edge set over these declarations that can be rendered: each owned
/// declaration (method, constructor, option builder, hook) has one owner, and
/// any subset of the other edges that [`is_valid`] accepts.
pub(super) fn edge_sets(decls: &[Decl]) -> Vec<Vec<Edge>> {
    let optional = candidates(decls);
    let mut sets = Vec::new();
    for owners in owners(decls) {
        for mask in 0..(1_u32 << optional.len()) {
            let mut edges = owners.clone();
            edges.extend(
                optional
                    .iter()
                    .enumerate()
                    .filter(|(bit, _)| mask & (1 << bit) != 0)
                    .map(|(_, edge)| *edge),
            );
            if is_valid(decls, &edges) {
                sets.push(edges);
            }
        }
    }
    sets
}

/// Every way to give each owned declaration an owner among the types: one
/// edge each. Empty when one has no type to belong to.
fn owners(decls: &[Decl]) -> Vec<Vec<Edge>> {
    let types: Vec<usize> = indices(decls, |kind| kind == Kind::Type).collect();
    let mut all = vec![Vec::new()];
    for owned in indices(decls, Kind::owned) {
        all = all
            .into_iter()
            .flat_map(|chosen| {
                types.iter().map(move |&owner| {
                    let mut next = chosen.clone();
                    next.push(Edge {
                        from: owned,
                        to: owner,
                        relation: Relation::Owner,
                    });
                    next
                })
            })
            .collect();
    }
    all
}

/// The edges a shape over these declarations may hold besides owners, before
/// the checks that need the whole shape.
fn candidates(decls: &[Decl]) -> Vec<Edge> {
    let mut edges = Vec::new();
    for from in 0..decls.len() {
        for to in (0..decls.len()).filter(|&to| to != from) {
            if let Some(relation) = relation(decls[from].kind, decls[to].kind) {
                edges.push(Edge { from, to, relation });
            }
        }
    }
    edges
}

/// The relation a declaration of kind `from` can have to one of kind `to`: a
/// function, method, constructor or `init` calls a function, constant or
/// variable (a method also the methods of its type) and names a type, and a
/// type names a type as a field.
fn relation(from: Kind, to: Kind) -> Option<Relation> {
    match (from.acts(), from, to) {
        (_, Kind::Method, Kind::Method) => Some(Relation::Calls),
        (true, _, Kind::Func | Kind::Const | Kind::Var) => Some(Relation::Calls),
        (true, _, Kind::Type) | (_, Kind::Type, Kind::Type) => Some(Relation::Uses),
        _ => None,
    }
}

/// Whether the edges make a shape that can be rendered: a method calls only
/// methods of its own type, a type has at most one constructor, option
/// builder and hook (a second would be the same name), and a tested method
/// belongs to a public type, as a test name needs.
fn is_valid(decls: &[Decl], edges: &[Edge]) -> bool {
    let owner = |owned: usize| {
        edges
            .iter()
            .find(|edge| edge.from == owned && edge.relation == Relation::Owner)
            .map(|edge| edge.to)
    };
    let own_methods = edges.iter().all(|edge| {
        let methods = decls[edge.from].kind == Kind::Method && decls[edge.to].kind == Kind::Method;
        !(methods && edge.relation == Relation::Calls) || owner(edge.from) == owner(edge.to)
    });
    let one_each = [Kind::Constructor, Kind::Option, Kind::Hook]
        .into_iter()
        .all(|kind| {
            let owners: Vec<Option<usize>> = indices(decls, |k| k == kind).map(owner).collect();
            owners
                .iter()
                .all(|o| owners.iter().filter(|p| *p == o).count() == 1)
        });
    let public_owner = indices(decls, |kind| kind == Kind::Method)
        .filter(|&i| decls[i].tested)
        .all(|i| owner(i).is_some_and(|ty| decls[ty].visibility == super::Visibility::Public));
    own_methods && one_each && public_owner
}

/// The indices of the declarations whose kind satisfies `which`.
fn indices<'a>(
    decls: &'a [Decl],
    which: impl Fn(Kind) -> bool + 'a,
) -> impl Iterator<Item = usize> + 'a {
    (0..decls.len()).filter(move |&i| which(decls[i].kind))
}
