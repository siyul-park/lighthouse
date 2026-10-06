use lighthouse_model::Position;
use lighthouse_syntax::{Error, Query, Syntax, children_named, descendants, span, text};

const SOURCE: &str = "package p\n\nfunc a() {}\n\nfunc b(x int) int { return x }\n";

fn go() -> Syntax {
    let mut syntax = Syntax::default();
    syntax.register("go", tree_sitter_go::LANGUAGE);
    syntax
}

#[test]
fn parses_registered_languages_only() {
    let syntax = go();
    let tree = syntax.parse("go", SOURCE).unwrap();
    assert_eq!(tree.root_node().kind(), "source_file");
    assert!(!tree.root_node().has_error());
    assert!(matches!(
        syntax.parse("rust", "fn main() {}"),
        Err(Error::UnknownLanguage(id)) if id == "rust"
    ));
}

#[test]
fn queries_return_captures_per_match_in_source_order() {
    let syntax = go();
    let tree = syntax.parse("go", SOURCE).unwrap();
    let query = Query::new(
        syntax.language("go").unwrap(),
        "(function_declaration name: (identifier) @name parameters: (parameter_list (parameter_declaration)* @param)) @fn",
    )
    .unwrap();
    let matches = query.matches(tree.root_node(), SOURCE);
    let names: Vec<_> = matches
        .iter()
        .filter_map(|m| m.get("name"))
        .map(|n| text(n, SOURCE))
        .collect();
    assert_eq!(names, ["a", "b"]);
    assert_eq!(matches[0].all("param").count(), 0);
    assert_eq!(matches[1].all("param").count(), 1);
    assert!(matches[0].get("missing").is_none());
}

#[test]
fn invalid_queries_are_reported() {
    let syntax = go();
    let err = Query::new(syntax.language("go").unwrap(), "(no_such_node) @x").err();
    assert!(matches!(err, Some(Error::Query(_))));
}

#[test]
fn spans_are_one_based_and_text_is_sliced_from_source() {
    let syntax = go();
    let tree = syntax.parse("go", SOURCE).unwrap();
    let funcs = children_named(tree.root_node(), "function_declaration");
    assert_eq!(funcs.len(), 2);
    let at = span(funcs[1]);
    assert_eq!(at.start, Position { line: 5, col: 1 });
    assert_eq!(at.end, Position { line: 5, col: 31 });
    assert_eq!(text(funcs[0], SOURCE), "func a() {}");
}

#[test]
fn descendants_walk_the_subtree_in_pre_order_without_the_root() {
    let syntax = go();
    let tree = syntax
        .parse("go", "package p\nfunc f() { g(1) }\n")
        .unwrap();
    let func = children_named(tree.root_node(), "function_declaration")[0];
    let kinds: Vec<_> = descendants(func).map(|n| n.kind()).collect();
    assert_eq!(kinds.first(), Some(&"func"));
    assert!(!kinds.is_empty());
    let calls = descendants(func).filter(|n| n.kind() == "call_expression");
    assert_eq!(calls.count(), 1);
    let outside: Vec<_> = descendants(func)
        .filter(|n| n.start_position().row != 1)
        .collect();
    assert!(outside.is_empty());
}
