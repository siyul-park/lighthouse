//! Lowering in a language that evaluates declarations as they are written: a
//! declaration is never put above a binding its code needs as the file loads,
//! nor a binding below the code that needs it.

use lighthouse_model::ucm::{ORDERED, When};

use super::*;

const LOADED: &str = "fn a() {}\n\nfn b() {}\n\nfn c() {}\n";

/// `LOADED` in a language that orders its declarations, where each of `uses`
/// (user, used, kind, when) is an edge between declarations.
fn ordered(uses: &[(&str, &str, EdgeKind, When)]) -> Case {
    let edges = uses
        .iter()
        .map(|(from, to, kind, when)| Edge {
            kind: *kind,
            from: id(from),
            to: Target::Resolved(id(to)),
            resolution: Resolution::Semantic,
            site: Some(span((1, 1), (1, 2))),
            when: *when,
        })
        .collect();
    let mut case = case(LOADED, &[("a", 1, 1), ("b", 3, 3), ("c", 5, 5)], edges);
    let mut fragment = Fragment {
        files: case.project.files.clone(),
        symbols: case.project.symbols.clone(),
        places: case.project.places.clone(),
        edges: case.project.edges.clone(),
        ..Fragment::default()
    };
    fragment.files[0]
        .facts
        .insert(ORDERED.to_owned(), String::new());
    case.project = Project::merge([fragment]);
    case
}

fn reorder(names: &[&str]) -> EditOp {
    EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: names.iter().map(|n| id(n)).collect(),
    }
}

fn before(node: &str, anchor: &str) -> EditOp {
    EditOp::Move {
        node: id(node),
        anchor: Anchor::Before(id(anchor)),
    }
}

#[test]
fn a_declaration_is_not_moved_above_a_binding_it_needs_at_load() {
    let case = ordered(&[("c", "a", EdgeKind::References, When::Load)]);

    let why = case.run(&[before("c", "a")]).unwrap_err();

    assert!(
        why.contains("load order") && why.contains("`c`") && why.contains("`a`"),
        "{why}"
    );
}

#[test]
fn a_binding_is_not_moved_below_the_code_that_needs_it_at_load() {
    let case = ordered(&[("c", "a", EdgeKind::Calls, When::Load)]);

    let why = case.run(&[after("a", "c")]).unwrap_err();

    assert!(why.contains("load order"), "{why}");
}

#[test]
fn a_use_that_runs_only_when_called_orders_nothing() {
    let case = ordered(&[("c", "a", EdgeKind::Calls, When::Call)]);

    assert_eq!(
        case.run(&[after("a", "c")]).unwrap(),
        "fn b() {}\n\nfn c() {}\n\nfn a() {}\n"
    );
}

#[test]
fn calling_a_function_at_load_needs_what_its_body_uses() {
    // `c` calls `b` as it loads, and `b` reads `a` when it runs.
    let case = ordered(&[
        ("c", "b", EdgeKind::Calls, When::Load),
        ("b", "a", EdgeKind::References, When::Call),
    ]);

    let why = case.run(&[before("c", "a")]).unwrap_err();

    assert!(why.contains("load order"), "{why}");
}

#[test]
fn a_language_without_ordered_declarations_moves_freely() {
    let edges = vec![Edge {
        kind: EdgeKind::References,
        from: id("c"),
        to: Target::Resolved(id("a")),
        resolution: Resolution::Semantic,
        site: None,
        when: When::Load,
    }];
    let case = case(LOADED, &[("a", 1, 1), ("b", 3, 3), ("c", 5, 5)], edges);

    assert_eq!(
        case.run(&[before("c", "a")]).unwrap(),
        "fn c() {}\n\nfn a() {}\n\nfn b() {}\n"
    );
}

#[test]
fn a_need_the_file_already_breaks_is_left_alone() {
    let case = ordered(&[("a", "c", EdgeKind::References, When::Load)]);

    assert_eq!(
        case.run(&[after("a", "c")]).unwrap(),
        "fn b() {}\n\nfn c() {}\n\nfn a() {}\n"
    );
}

#[test]
fn a_reorder_follows_the_needs_among_the_declarations_it_lists() {
    // Wanted `c`, `b`, `a`, but `c` needs `a` and `b` needs `a`: `a` goes first.
    let case = ordered(&[
        ("c", "a", EdgeKind::References, When::Load),
        ("b", "a", EdgeKind::References, When::Load),
    ]);

    assert_eq!(
        case.run(&[reorder(&["c", "b", "a"])]).unwrap(),
        "fn a() {}\n\nfn c() {}\n\nfn b() {}\n"
    );
}
