//! Lowering a move: the block travels with its comments and keeps the blank lines around it.

use super::*;

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
