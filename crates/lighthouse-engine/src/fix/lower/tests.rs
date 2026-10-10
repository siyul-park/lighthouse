//! Lowering: moves, deletes, reorders, renames and the refusals around them.

use std::fs;

use lighthouse_model::{Edge, File, Fragment, Position, Span, SymbolKind, Visibility};

use super::*;
use crate::fix::edits::apply;

const FILE: &str = "a.src";

fn span(from: (u32, u32), to: (u32, u32)) -> Span {
    Span {
        start: Position {
            line: from.0,
            col: from.1,
        },
        end: Position {
            line: to.0,
            col: to.1,
        },
    }
}

/// A declaration that fills whole lines `from..=to` of `text`.
fn symbol(text: &str, name: &str, from: u32, to: u32) -> Symbol {
    let last = text.lines().nth(to as usize - 1).unwrap_or_default();
    let extent = span((from, 1), (to, u32::try_from(last.len()).unwrap() + 1));
    Symbol {
        id: SymbolId::new("m", &[], name, SymbolKind::Function),
        kind: SymbolKind::Function,
        visibility: Visibility::Private,
        owner: None,
        file: FILE.into(),
        span: extent,
        extent: Some(extent),
        doc: None,
        name: name.to_owned(),
        role: None,
        optional: false,
    }
}

fn id(name: &str) -> Node {
    Node::Symbol(SymbolId::new("m", &[], name, SymbolKind::Function))
}

struct Case {
    dir: tempfile::TempDir,
    project: Project,
}

fn case(text: &str, symbols: &[(&str, u32, u32)], edges: Vec<Edge>) -> Case {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(FILE), text).unwrap();
    let project = Project::merge([Fragment {
        files: vec![File {
            path: FILE.into(),
            lang: "src".to_owned(),
            hash: String::new(),
            generated: false,
            test: false,
        }],
        symbols: symbols
            .iter()
            .map(|(name, from, to)| symbol(text, name, *from, *to))
            .collect(),
        edges,
        ..Fragment::default()
    }]);
    Case { dir, project }
}

impl Case {
    /// The file after lowering and applying `ops`, or why they were refused.
    fn run(&self, ops: &[EditOp]) -> Result<String, String> {
        let overlays = Overlays::new();
        let sources = Sources::new(self.dir.path(), &self.project, &overlays);
        let complete = |_: &str| true;
        let lowerer = Lowerer::new(&self.project, &sources, &complete);
        let mut edits = Vec::new();
        for op in ops {
            edits.extend(lowerer.lower(op)?);
        }
        let text = sources.text(Path::new(FILE))?;
        apply(&text, &edits.iter().collect::<Vec<_>>())
    }
}

fn after(node: &str, anchor: &str) -> EditOp {
    EditOp::Move {
        node: id(node),
        anchor: Anchor::After(id(anchor)),
    }
}

const THREE: &str = "fn a() {}\n\nfn b() {}\n\nfn c() {}\n";

#[test]
fn a_move_after_keeps_blank_line_separation() {
    let case = case(THREE, &[("a", 1, 1), ("b", 3, 3), ("c", 5, 5)], vec![]);

    let moved = case.run(&[after("a", "c")]).unwrap();

    assert_eq!(moved, "fn b() {}\n\nfn c() {}\n\nfn a() {}\n");
}

#[test]
fn a_move_before_puts_the_block_and_a_blank_line_ahead() {
    let case = case(THREE, &[("a", 1, 1), ("b", 3, 3), ("c", 5, 5)], vec![]);
    let op = EditOp::Move {
        node: id("c"),
        anchor: Anchor::Before(id("a")),
    };

    assert_eq!(
        case.run(&[op]).unwrap(),
        "fn c() {}\n\nfn a() {}\n\nfn b() {}\n"
    );
}

#[test]
fn a_move_takes_leading_comment_lines_of_the_extent_along() {
    let text = "fn a() {}\n\n// b does it.\nfn b() {}\n\nfn c() {}\n";
    let case = case(text, &[("a", 1, 1), ("b", 3, 4), ("c", 6, 6)], vec![]);

    let moved = case.run(&[after("b", "c")]).unwrap();

    assert_eq!(
        moved,
        "fn a() {}\n\nfn c() {}\n\n// b does it.\nfn b() {}\n"
    );
}

#[test]
fn a_move_of_the_last_block_without_a_final_newline_stays_well_formed() {
    let case = case(
        "fn a() {}\n\nfn b() {}",
        &[("a", 1, 1), ("b", 3, 3)],
        vec![],
    );
    let op = EditOp::Move {
        node: id("b"),
        anchor: Anchor::Before(id("a")),
    };

    assert_eq!(case.run(&[op]).unwrap(), "fn b() {}\n\nfn a() {}\n");
}

#[test]
fn moving_next_to_itself_or_a_missing_symbol_is_refused() {
    let case = case(THREE, &[("a", 1, 1), ("b", 3, 3)], vec![]);

    assert!(case.run(&[after("a", "a")]).is_err());
    assert!(
        case.run(&[after("a", "zzz")])
            .unwrap_err()
            .contains("not in the project")
    );
}

#[test]
fn a_symbol_without_an_extent_cannot_move() {
    let mut case = case(THREE, &[("a", 1, 1), ("b", 3, 3)], vec![]);
    let mut fragment = Fragment {
        files: case.project.files.clone(),
        symbols: case.project.symbols.clone(),
        ..Fragment::default()
    };
    fragment.symbols[0].extent = None;
    case.project = Project::merge([fragment]);

    let why = case.run(&[after("a", "b")]).unwrap_err();

    assert!(why.contains("no extent"), "{why}");
}

#[test]
fn declarations_in_different_blocks_do_not_move_next_to_each_other() {
    let text = "impl A {\n    fn a() {}\n}\n\nimpl B {\n    fn b() {}\n}\n";
    let case = case(text, &[("a", 2, 2), ("b", 6, 6)], vec![]);

    let why = case.run(&[after("a", "b")]).unwrap_err();

    assert!(why.contains("not in the same block"), "{why}");
}

#[test]
fn declarations_in_one_block_move_within_it() {
    let text = "impl A {\n    fn a() {}\n\n    fn b() {}\n}\n";
    let case = case(text, &[("a", 2, 2), ("b", 4, 4)], vec![]);

    let moved = case.run(&[after("a", "b")]).unwrap();

    assert_eq!(moved, "impl A {\n    fn b() {}\n\n    fn a() {}\n}\n");
}

#[test]
fn a_member_of_a_parenthesized_group_never_moves() {
    let text = "const (\n    a = 1\n    b = 2\n)\n\nfn c() {}\n";
    let case = case(text, &[("a", 2, 2), ("b", 3, 3), ("c", 6, 6)], vec![]);

    let why = case.run(&[after("a", "b")]).unwrap_err();

    assert!(why.contains("parenthesized group"), "{why}");
}

#[test]
fn a_declaration_sharing_its_line_with_other_code_cannot_move_alone() {
    let text = "fn a() {} fn b() {}\n\nfn c() {}\n";
    let mut case = case(text, &[("b", 1, 1), ("c", 3, 3)], vec![]);
    let mut fragment = Fragment {
        files: case.project.files.clone(),
        symbols: case.project.symbols.clone(),
        ..Fragment::default()
    };
    fragment.symbols[0].extent = Some(span((1, 11), (1, 20)));
    case.project = Project::merge([fragment]);

    let why = case.run(&[after("b", "c")]).unwrap_err();

    assert!(why.contains("shares its lines"), "{why}");
}

#[test]
fn delete_removes_the_block_and_one_blank_line() {
    let case = case(THREE, &[("a", 1, 1), ("b", 3, 3), ("c", 5, 5)], vec![]);

    let deleted = case.run(&[EditOp::Delete { node: id("b") }]).unwrap();

    assert_eq!(deleted, "fn a() {}\n\nfn c() {}\n");
}

#[test]
fn reorder_permutes_the_blocks_within_the_places_they_occupy() {
    let case = case(THREE, &[("a", 1, 1), ("b", 3, 3), ("c", 5, 5)], vec![]);
    let op = EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: vec![id("c"), id("a"), id("b")],
    };

    assert_eq!(
        case.run(&[op]).unwrap(),
        "fn c() {}\n\nfn a() {}\n\nfn b() {}\n"
    );
}

#[test]
fn reorder_leaves_declarations_it_does_not_list_in_their_places() {
    let case = case(THREE, &[("a", 1, 1), ("b", 3, 3), ("c", 5, 5)], vec![]);
    let op = EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: vec![id("c"), id("a")],
    };

    assert_eq!(
        case.run(&[op]).unwrap(),
        "fn c() {}\n\nfn b() {}\n\nfn a() {}\n"
    );
}

#[test]
fn reorder_sorts_each_block_on_its_own() {
    let text = "fn b() {}\n\nimpl X {\n    fn d() {}\n\n    fn c() {}\n}\n\nfn a() {}\n";
    let case = case(
        text,
        &[("b", 1, 1), ("d", 4, 4), ("c", 6, 6), ("a", 9, 9)],
        vec![],
    );
    let op = EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: vec![id("a"), id("b"), id("c"), id("d")],
    };

    let sorted = case.run(&[op]).unwrap();

    assert_eq!(
        sorted,
        "fn a() {}\n\nimpl X {\n    fn c() {}\n\n    fn d() {}\n}\n\nfn b() {}\n"
    );
}

#[test]
fn delete_range_takes_whole_lines_when_the_range_fills_them() {
    let case = case("// note\nfn a() {}\n", &[("a", 2, 2)], vec![]);
    let op = EditOp::DeleteRange {
        file: FILE.into(),
        span: span((1, 1), (1, 8)),
    };

    assert_eq!(case.run(&[op]).unwrap(), "fn a() {}\n");
}

#[test]
fn delete_range_of_a_trailing_comment_leaves_the_code() {
    let case = case("fn a() {} // note\n", &[("a", 1, 1)], vec![]);
    let op = EditOp::DeleteRange {
        file: FILE.into(),
        span: span((1, 11), (1, 18)),
    };

    assert_eq!(case.run(&[op]).unwrap(), "fn a() {}\n");
}

#[test]
fn replace_inserts_at_an_empty_range() {
    let case = case("fn a() {}\n", &[("a", 1, 1)], vec![]);
    let op = EditOp::Replace {
        file: FILE.into(),
        span: span((1, 1), (1, 1)),
        text: "// hi\n".to_owned(),
    };

    assert_eq!(case.run(&[op]).unwrap(), "// hi\nfn a() {}\n");
}

#[test]
fn a_span_outside_the_file_is_refused() {
    let case = case("fn a() {}\n", &[("a", 1, 1)], vec![]);
    let op = EditOp::DeleteRange {
        file: FILE.into(),
        span: span((9, 1), (9, 2)),
    };

    assert!(case.run(&[op]).unwrap_err().contains("outside the file"));
}

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

#[test]
fn a_korean_banner_and_a_multibyte_end_do_not_split_a_character() {
    let text = "// 도우미 함수들\nfn a() {} // 끝\n\nfn b() { \"한\" }\n";
    let mut case = case(text, &[("a", 2, 2), ("b", 4, 4)], vec![]);
    // `b`'s extent ends right after the multibyte `}` neighbour: a span
    // whose last byte is inside a character must not panic.
    let mut fragment = Fragment {
        files: case.project.files.clone(),
        symbols: case.project.symbols.clone(),
        ..Fragment::default()
    };
    fragment.symbols[1].extent = Some(span((4, 1), (4, 17)));
    case.project = Project::merge([fragment]);

    let moved = case.run(&[after("a", "b")]).unwrap();

    assert_eq!(
        moved,
        "// 도우미 함수들\nfn b() { \"한\" }\n\nfn a() {} // 끝\n"
    );
    let deleted = case.run(&[EditOp::DeleteRange {
        file: FILE.into(),
        span: span((1, 1), (1, 23)),
    }]);
    assert_eq!(deleted.unwrap(), "fn a() {} // 끝\n\nfn b() { \"한\" }\n");
}

#[test]
fn an_extent_that_ends_inside_a_character_is_refused_not_a_panic() {
    let text = "fn a() { \"한\" }\n\nfn b() {}\n";
    let mut case = case(text, &[("a", 1, 1), ("b", 3, 3)], vec![]);
    let mut fragment = Fragment {
        files: case.project.files.clone(),
        symbols: case.project.symbols.clone(),
        ..Fragment::default()
    };
    fragment.symbols[0].extent = Some(span((1, 1), (1, 12)));
    case.project = Project::merge([fragment]);

    let why = case.run(&[after("a", "b")]).unwrap_err();

    assert!(why.contains("outside its file"), "{why}");
}

#[test]
fn crlf_text_gets_crlf_where_a_move_inserts() {
    let text = "fn a() {}\r\n\r\nfn b() {}\r\n\r\nfn c() {}\r\n";
    let case = case(text, &[("a", 1, 1), ("b", 3, 3), ("c", 5, 5)], vec![]);

    let moved = case.run(&[after("a", "c")]).unwrap();

    assert_eq!(moved, "fn b() {}\r\n\r\nfn c() {}\r\n\r\nfn a() {}\r\n");
}

#[test]
fn a_block_comment_after_the_closing_brace_shares_the_line() {
    let text = "fn a() {} /* open\n still */ fn x() {}\n\nfn c() {}\n";
    let mut case = case(text, &[("a", 1, 1), ("c", 4, 4)], vec![]);
    let mut fragment = Fragment {
        files: case.project.files.clone(),
        symbols: case.project.symbols.clone(),
        ..Fragment::default()
    };
    fragment.symbols[0].extent = Some(span((1, 1), (1, 10)));
    case.project = Project::merge([fragment]);

    let why = case.run(&[after("a", "c")]).unwrap_err();

    assert!(why.contains("shares its lines"), "{why}");
}

fn contained(file: &str) -> Result<String, String> {
    let case = case(THREE, &[("a", 1, 1), ("b", 3, 3)], vec![]);
    case.run(&[EditOp::Replace {
        file: file.into(),
        span: span((1, 1), (1, 1)),
        text: "x".to_owned(),
    }])
}

#[test]
fn edits_stay_inside_relative_normalized_project_files() {
    assert!(
        contained("../a.src")
            .unwrap_err()
            .contains("not a relative, normalized")
    );
    assert!(
        contained("/etc/passwd")
            .unwrap_err()
            .contains("not a relative, normalized")
    );
    assert!(
        contained("./a.src")
            .unwrap_err()
            .contains("not a relative, normalized")
    );
    assert!(
        contained("other.src")
            .unwrap_err()
            .contains("not a file of the project")
    );
}

#[test]
fn generated_files_are_not_edited() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(FILE), "x\n").unwrap();
    let project = Project::merge([Fragment {
        files: vec![File {
            path: FILE.into(),
            lang: "src".to_owned(),
            hash: String::new(),
            generated: true,
            test: false,
        }],
        ..Fragment::default()
    }]);
    let overlays = Overlays::new();
    let sources = Sources::new(dir.path(), &project, &overlays);

    assert!(
        sources
            .text(Path::new(FILE))
            .unwrap_err()
            .contains("generated")
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_file_or_directory_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("x.src"), "x\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("sub")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("x.src"), dir.path().join("link.src")).unwrap();
    let file = |path: &str| File {
        path: path.into(),
        lang: "src".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
    };
    let project = Project::merge([Fragment {
        files: vec![file("sub/x.src"), file("link.src")],
        ..Fragment::default()
    }]);
    let overlays = Overlays::new();
    let sources = Sources::new(dir.path(), &project, &overlays);

    for path in ["sub/x.src", "link.src"] {
        let why = sources.text(Path::new(path)).unwrap_err();
        assert!(why.contains("symlink"), "{path}: {why}");
    }
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
