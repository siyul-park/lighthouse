use super::*;

fn texts(source: &str) -> Vec<String> {
    scan(source, &[]).into_iter().map(|c| c.text).collect()
}

#[test]
fn adjacent_line_comments_form_one_comment() {
    assert_eq!(
        texts("// a\n// b\nfn f() {}\n\n// c\n"),
        ["// a\n// b", "// c"]
    );
}

#[test]
fn doc_and_plain_runs_stay_apart() {
    assert_eq!(texts("/// a\n// b\n"), ["/// a", "// b"]);
}

#[test]
fn trailing_comments_do_not_join_the_next_line() {
    assert_eq!(texts("let x = 1; // a\n// b\n"), ["// a", "// b"]);
}

#[test]
fn literals_hide_comment_markers() {
    let source =
        "let a = \"// no\"; let b = r#\"/* no\"#; let c = '/'; let d: &'a str; // yes\n";
    assert_eq!(texts(source), ["// yes"]);
}

#[test]
fn block_comments_nest_and_keep_their_span() {
    let found = scan("x\n/* a /* b */ c */\n", &[]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].text, "/* a /* b */ c */");
    assert_eq!((found[0].span.start.line, found[0].span.start.col), (2, 1));
    assert_eq!((found[0].span.end.line, found[0].span.end.col), (2, 18));
}

fn symbol_at(from: (u32, u32), to: (u32, u32)) -> Symbol {
    let at = |(line, col)| Position { line, col };
    let span = Span {
        start: at(from),
        end: at(to),
    };
    Symbol {
        id: "m::f#function".to_owned(),
        kind: lighthouse_protocol::SymbolKind::Function,
        visibility: lighthouse_protocol::Visibility::Private,
        owner: None,
        file: "a.rs".to_owned(),
        span,
        extent: Some(span),
        doc: None,
        name: "f".to_owned(),
        role: None,
        optional: false,
        type_ref: None,
    }
}

#[test]
fn an_extent_grows_over_every_standalone_comment_that_leads_it() {
    let source = "// far\n\n// a\n// b\n/// doc\nfn f() {}\n";
    let mut symbols = [symbol_at((5, 1), (6, 8))];

    widen_extents(source, &scan(source, &symbols), &mut symbols);

    let start = symbols[0].extent.unwrap().start;
    assert_eq!((start.line, start.col), (3, 1), "the blank line stops it");
}

#[test]
fn inner_docs_and_banners_stay_out_of_an_extent() {
    let inner = "//! module docs\nfn f() {}\n";
    let banner = "// ---- Helpers ----\nfn f() {}\n";
    let marker = "// MARK: - Helpers\nfn f() {}\n";
    let prose = "// ---- not a banner: it explains\n// the item\nfn f() {}\n";

    for (source, line, want) in [(inner, 2, 2), (banner, 2, 2), (marker, 2, 2), (prose, 3, 1)] {
        let mut symbols = [symbol_at((line, 1), (line, 8))];
        widen_extents(source, &scan(source, &symbols), &mut symbols);
        assert_eq!(symbols[0].extent.unwrap().start.line, want, "{source}");
    }
}

#[test]
fn a_comment_that_trails_code_does_not_belong_to_the_next_item() {
    let source = "let x = 1; // trailing\nfn f() {}\n";
    let mut symbols = [symbol_at((2, 1), (2, 10))];

    widen_extents(source, &scan(source, &symbols), &mut symbols);

    assert_eq!(symbols[0].extent.unwrap().start.line, 2);
}
