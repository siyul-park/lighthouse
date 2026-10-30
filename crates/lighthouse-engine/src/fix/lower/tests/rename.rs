//! Lowering a rename: the declaration and every site, refused when a name would clash.

use super::*;

fn reference(from: &str, to: &str, site: Option<Span>, resolution: Resolution) -> Edge {
    let Node::Symbol(from) = id(from) else {
        unreachable!()
    };
    Edge {
        kind: EdgeKind::Calls,
        from: Node::Symbol(from),
        to: Target::Path(format!("m::{to}")),
        resolution,
        site,
    }
}

const RENAME: &str = "fn old() {}\n\nfn user() { old(); old() }\n";

fn rename(to: &str) -> EditOp {
    let Node::Symbol(symbol) = id("old") else {
        unreachable!()
    };
    EditOp::Rename {
        symbol,
        name: to.to_owned(),
    }
}

#[test]
fn rename_edits_the_declaration_and_every_site() {
    let edges = vec![
        reference(
            "user",
            "old",
            Some(span((3, 13), (3, 16))),
            Resolution::Semantic,
        ),
        reference(
            "user",
            "old",
            Some(span((3, 20), (3, 23))),
            Resolution::Semantic,
        ),
    ];
    let case = case(RENAME, &[("old", 1, 1), ("user", 3, 3)], edges);

    let renamed = case.run(&[rename("fresh")]).unwrap();

    assert_eq!(renamed, "fn fresh() {}\n\nfn user() { fresh(); fresh() }\n");
}

#[test]
fn rename_is_refused_when_a_reference_has_no_site() {
    let edges = vec![reference("user", "old", None, Resolution::Semantic)];
    let case = case(RENAME, &[("old", 1, 1), ("user", 3, 3)], edges);

    let why = case.run(&[rename("fresh")]).unwrap_err();

    assert!(why.contains("without a site"), "{why}");
}

#[test]
fn rename_is_refused_when_a_reference_is_not_semantic() {
    let edges = vec![reference(
        "user",
        "old",
        Some(span((3, 13), (3, 16))),
        Resolution::Syntactic,
    )];
    let case = case(RENAME, &[("old", 1, 1), ("user", 3, 3)], edges);

    let why = case.run(&[rename("fresh")]).unwrap_err();

    assert!(why.contains("not semantic"), "{why}");
}

#[test]
fn rename_refuses_names_that_are_not_identifiers() {
    let case = case(RENAME, &[("old", 1, 1), ("user", 3, 3)], vec![]);

    assert!(case.run(&[rename("two words")]).is_err());
    assert!(case.run(&[rename("old")]).is_err());
}

#[test]
fn the_name_of_a_go_method_is_found_after_its_receiver() {
    assert_eq!(name_in("func (s *Run) Run() int {", "Run"), Some(14));
    assert_eq!(name_in("fn run_all() {}", "run"), None);
}

fn renamed(visibility: Visibility, to: &str, complete: bool) -> Result<String, String> {
    let mut case = case(RENAME, &[("old", 1, 1), ("user", 3, 3)], vec![]);
    let mut fragment = Fragment {
        files: case.project.files.clone(),
        symbols: case.project.symbols.clone(),
        ..Fragment::default()
    };
    fragment.symbols[0].visibility = visibility;
    case.project = Project::merge([fragment]);
    let overlays = Overlays::new();
    let sources = Sources::new(case.dir.path(), &case.project, &overlays);
    let provides = |_: &str| complete;
    let lowerer = Lowerer::new(&case.project, &sources, &provides);
    lowerer.lower(&rename(to)).map(|_| String::new())
}

#[test]
fn rename_needs_complete_references_a_private_symbol_and_a_free_name() {
    assert!(renamed(Visibility::Private, "fresh", true).is_ok());
    let missing = renamed(Visibility::Private, "fresh", false).unwrap_err();
    assert!(missing.contains("complete-references"), "{missing}");
    let public = renamed(Visibility::Public, "fresh", true).unwrap_err();
    assert!(public.contains("visible outside"), "{public}");
    let taken = renamed(Visibility::Private, "user", true).unwrap_err();
    assert!(taken.contains("already declared"), "{taken}");
    for bad in ["fn", "9lives", "two words", ""] {
        let why = renamed(Visibility::Private, bad, true).unwrap_err();
        assert!(why.contains("not a usable identifier"), "{bad}: {why}");
    }
}

/// A (line, column) pair.
type Point = (u32, u32);

fn site_edges(sites: &[(Point, Point)]) -> Vec<Edge> {
    sites
        .iter()
        .map(|(from, to)| reference("user", "old", Some(span(*from, *to)), Resolution::Semantic))
        .collect()
}

#[test]
fn rename_is_refused_when_a_caller_has_a_local_of_the_new_name() {
    let text = "fn old() {}\n\nfn user() { let fresh = 1; old(); old() }\n";
    let edges = site_edges(&[((3, 28), (3, 31)), ((3, 35), (3, 38))]);
    let case = case(text, &[("old", 1, 1), ("user", 3, 3)], edges);

    let why = case.run(&[rename("fresh")]).unwrap_err();

    assert!(why.contains("capture or shadow"), "{why}");
    assert!(case.run(&[rename("other")]).is_ok(), "a free name is fine");
}

#[test]
fn rename_is_refused_when_the_new_name_is_declared_where_a_site_is() {
    let text = "fn old() {}\n\nfn fresh() {}\n\nfn user() { old() }\n";
    let edges = site_edges(&[((5, 13), (5, 16))]);
    let case = case(
        text,
        &[("old", 1, 1), ("fresh", 3, 3), ("user", 5, 5)],
        edges,
    );

    let why = case.run(&[rename("fresh")]).unwrap_err();

    assert!(why.contains("fresh"), "{why}");
}
