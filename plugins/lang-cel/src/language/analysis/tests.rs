use std::collections::BTreeMap;

use lighthouse_model::{
    Span,
    ucm::{Edge, EdgeKind, Resolution, SymbolId, SymbolKind, Target},
};

use super::*;
use crate::language::source::Host;

/// `text` as an expression that starts at the first byte of its host file.
fn mapped(text: &str) -> MappedText {
    MappedText {
        text: text.to_owned(),
        starts: (0..text.len()).collect(),
        ends: (1..=text.len()).collect(),
        source_start: 0,
        source_end: text.len(),
        host: Host::new(text),
    }
}

fn edges_of(text: &str) -> Vec<Edge> {
    let program = cel::Env::stdlib().compile(text).unwrap();
    let packs = PackVariables::new();
    let mut external = BTreeMap::new();
    let from = SymbolId::new("cel", &["doc"], "expression", SymbolKind::Function);
    let (_, edges) = Analysis::new(&[], &packs, &[], &mut external).run(
        &program,
        &mapped(text),
        &from,
        "doc",
        &[],
    );
    edges
}

/// The text a span covers on the first line of `text`.
fn covered(text: &str, span: Span) -> &str {
    assert_eq!(span.start.line, 1);
    assert_eq!(span.end.line, 1);
    &text[span.start.col as usize - 1..span.end.col as usize - 1]
}

fn site_texts(text: &str, kind: EdgeKind) -> Vec<String> {
    edges_of(text)
        .iter()
        .filter(|edge| edge.kind == kind)
        .map(|edge| covered(text, edge.site.unwrap()).to_owned())
        .collect()
}

#[test]
fn a_reference_site_covers_the_identifier_and_nothing_past_it() {
    assert_eq!(
        site_texts("alpha + beta", EdgeKind::References),
        ["alpha", "beta"]
    );
}

#[test]
fn a_call_site_is_the_name_of_the_function() {
    assert_eq!(site_texts("size(name)", EdgeKind::Calls), ["size"]);
    assert_eq!(
        site_texts("name.startsWith('a')", EdgeKind::Calls),
        ["startsWith"]
    );
}

#[test]
fn a_field_site_is_the_field_without_its_dot() {
    assert_eq!(
        site_texts("rec.name.first", EdgeKind::References),
        ["rec", "name", "first"]
    );
}

#[test]
fn a_site_after_non_ascii_text_stays_on_its_bytes() {
    let sites = site_texts("'é' == other", EdgeKind::References);
    assert_eq!(sites, ["other"]);
}

#[test]
fn tokens_are_counted_by_characters() {
    assert_eq!(metrics::token_count("é + 'ü'"), 3);
    assert_eq!(metrics::token_count("a && b == 'x y'"), 5);
    assert_eq!(metrics::token_count("  "), 0);
}

#[test]
fn the_namespaces_of_the_tables_are_those_of_the_environment() {
    let cel = lighthouse_spec::Env::standard().unwrap();
    for namespace in names::namespaces() {
        assert!(cel.is_namespace(namespace), "{namespace}");
    }
}

#[test]
fn a_pack_variable_resolves_from_a_decision_of_the_pack() {
    let pack_variable = SymbolId::new("cel", &["Pack:design", "p"], "expression", SymbolKind::Var);
    let packs: PackVariables = [(
        "shared".to_owned(),
        vec![("Pack:design".to_owned(), pack_variable.clone(), false)],
    )]
    .into();
    let analysis = |doc: &str| {
        let text = "shared";
        let program = cel::Env::stdlib().compile(text).unwrap();
        let mut external = BTreeMap::new();
        let from = SymbolId::new("cel", &[doc], "expression", SymbolKind::Function);
        let (_, edges) = Analysis::new(&[], &packs, &[], &mut external).run(
            &program,
            &mapped(text),
            &from,
            doc,
            &[],
        );
        edges
    };
    let target = |edges: &[Edge]| match &edges[0] {
        Edge {
            to: Target::Path(path),
            resolution,
            ..
        } => (path.clone(), *resolution),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        target(&analysis("Decision:design/x")),
        (pack_variable.as_str().to_owned(), Resolution::Syntactic)
    );
    let (path, resolution) = target(&analysis("Decision:testing/x"));
    assert!(path.starts_with("cel/unresolved"), "{path}");
    assert_eq!(resolution, Resolution::Heuristic);
}
