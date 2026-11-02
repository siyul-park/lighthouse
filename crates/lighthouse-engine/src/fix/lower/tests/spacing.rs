//! Lowering keeps the separation of a scope: after a move or a reorder every
//! gap between declarations is the one the scope used, no run of blank lines
//! grows, and the text ends as it did.

use super::*;

const PYTHON: &str = "def a():\n    pass\n\n\ndef b():\n    pass\n\n\ndef c():\n    pass\n";
const PYTHON_SYMBOLS: &[(&str, u32, u32)] = &[("a", 1, 2), ("b", 5, 6), ("c", 9, 10)];

fn reorder(order: &[&str]) -> EditOp {
    EditOp::Reorder {
        owner: Owner::File(FILE.into()),
        order: order.iter().map(|name| id(name)).collect(),
    }
}

/// The text after the reorder to `order`, and after the same reorder over
/// that text, whose declarations stand at `again`.
fn twice(
    text: &str,
    symbols: &[(&str, u32, u32)],
    order: &[&str],
    again: &[(&str, u32, u32)],
) -> (String, String) {
    let once = case(text, symbols, vec![]).run(&[reorder(order)]).unwrap();
    let twice = case(&once, again, vec![]).run(&[reorder(order)]).unwrap();
    (once, twice)
}

#[test]
fn a_move_to_the_end_keeps_two_blank_lines_between_module_declarations() {
    let case = case(PYTHON, PYTHON_SYMBOLS, vec![]);

    assert_eq!(
        case.run(&[after("a", "c")]).unwrap(),
        "def b():\n    pass\n\n\ndef c():\n    pass\n\n\ndef a():\n    pass\n"
    );
}

#[test]
fn a_move_of_the_last_declaration_to_the_front_keeps_the_separation_and_the_end() {
    let case = case(PYTHON, PYTHON_SYMBOLS, vec![]);
    let op = EditOp::Move {
        node: id("c"),
        anchor: Anchor::Before(id("a")),
    };

    assert_eq!(
        case.run(&[op]).unwrap(),
        "def c():\n    pass\n\n\ndef a():\n    pass\n\n\ndef b():\n    pass\n"
    );
}

#[test]
fn a_move_from_the_middle_leaves_one_gap_where_there_were_two_bridged() {
    let case = case(PYTHON, PYTHON_SYMBOLS, vec![]);

    assert_eq!(
        case.run(&[after("b", "c")]).unwrap(),
        "def a():\n    pass\n\n\ndef c():\n    pass\n\n\ndef b():\n    pass\n"
    );
}

#[test]
fn a_reorder_of_module_declarations_keeps_two_blank_lines_and_is_stable() {
    let ranks = [("c", 1, 2), ("a", 5, 6), ("b", 9, 10)];

    let (once, again) = twice(PYTHON, PYTHON_SYMBOLS, &["c", "a", "b"], &ranks);

    assert_eq!(
        once,
        "def c():\n    pass\n\n\ndef a():\n    pass\n\n\ndef b():\n    pass\n"
    );
    assert_eq!(again, once);
}

#[test]
fn a_leading_hash_comment_travels_with_its_declaration() {
    let text = "def a():\n    pass\n\n\n# b reads.\ndef b():\n    pass\n\n\ndef c():\n    pass\n";
    let case = case(text, &[("a", 1, 2), ("b", 6, 7), ("c", 10, 11)], vec![]);

    assert_eq!(
        case.run(&[after("b", "c")]).unwrap(),
        "def a():\n    pass\n\n\ndef c():\n    pass\n\n\n# b reads.\ndef b():\n    pass\n"
    );
}

#[test]
fn an_insert_before_the_first_kept_declaration_goes_above_its_comment() {
    let text = "// c.\nfn c() {}\n\n// a.\nfn a() {}\n\n// b.\nfn b() {}\n";
    let case = case(text, &[("c", 2, 2), ("a", 5, 5), ("b", 8, 8)], vec![]);

    assert_eq!(
        case.run(&[reorder(&["b", "c", "a"])]).unwrap(),
        "// b.\nfn b() {}\n\n// c.\nfn c() {}\n\n// a.\nfn a() {}\n"
    );
}

#[test]
fn members_with_one_blank_line_in_a_class_keep_one() {
    let text = "class K:\n    def a(self):\n        pass\n\n    def b(self):\n        pass\n\n    def c(self):\n        pass\n\n\nVALUE = 1\n";
    let case = case(
        text,
        &[("a", 2, 3), ("b", 5, 6), ("c", 8, 9)],
        vec![],
    );

    assert_eq!(
        case.run(&[after("c", "a")]).unwrap(),
        "class K:\n    def a(self):\n        pass\n\n    def c(self):\n        pass\n\n    def b(self):\n        pass\n\n\nVALUE = 1\n"
    );
    assert_eq!(
        case.run(&[after("a", "c")]).unwrap(),
        "class K:\n    def b(self):\n        pass\n\n    def c(self):\n        pass\n\n    def a(self):\n        pass\n\n\nVALUE = 1\n"
    );
}

#[test]
fn contiguous_declarations_stay_contiguous() {
    let text = "const a = 1;\nconst b = 2;\nconst c = 3;\n";
    let case = case(text, &[("a", 1, 1), ("b", 2, 2), ("c", 3, 3)], vec![]);

    assert_eq!(
        case.run(&[after("a", "c")]).unwrap(),
        "const b = 2;\nconst c = 3;\nconst a = 1;\n"
    );
    assert_eq!(
        case.run(&[reorder(&["c", "a", "b"])]).unwrap(),
        "const c = 3;\nconst a = 1;\nconst b = 2;\n"
    );
}

#[test]
fn a_declaration_set_apart_stays_set_apart_next_to_a_contiguous_pair() {
    let text = "const a = 1;\nconst b = 2;\n\nfn c() {}\n";
    let case = case(text, &[("a", 1, 1), ("b", 2, 2), ("c", 4, 4)], vec![]);

    assert_eq!(
        case.run(&[after("b", "c")]).unwrap(),
        "const a = 1;\n\nfn c() {}\n\nconst b = 2;\n"
    );
}

#[test]
fn crlf_keeps_its_separation_and_its_line_ends() {
    let text = PYTHON.replace('\n', "\r\n");
    let case = case(&text, PYTHON_SYMBOLS, vec![]);

    assert_eq!(
        case.run(&[after("a", "c")]).unwrap(),
        "def b():\r\n    pass\r\n\r\n\r\ndef c():\r\n    pass\r\n\r\n\r\ndef a():\r\n    pass\r\n"
    );
}

#[test]
fn trailing_blank_lines_of_the_file_stay_when_the_last_declaration_moves() {
    let text = "fn a() {}\n\nfn b() {}\n\n";
    let case = case(text, &[("a", 1, 1), ("b", 3, 3)], vec![]);
    let op = EditOp::Move {
        node: id("b"),
        anchor: Anchor::Before(id("a")),
    };

    assert_eq!(case.run(&[op]).unwrap(), "fn b() {}\n\nfn a() {}\n\n");
}

#[test]
fn a_header_above_the_first_declaration_keeps_its_gap() {
    let text = "package demo\n\nfunc a() {}\n\nfunc b() {}\n";
    let case = case(text, &[("a", 3, 3), ("b", 5, 5)], vec![]);

    assert_eq!(
        case.run(&[after("a", "b")]).unwrap(),
        "package demo\n\nfunc b() {}\n\nfunc a() {}\n"
    );
}

#[test]
fn deleting_between_two_blank_lines_never_leaves_three() {
    let case = case(PYTHON, PYTHON_SYMBOLS, vec![]);

    assert_eq!(
        case.run(&[EditOp::Delete { node: id("b") }]).unwrap(),
        "def a():\n    pass\n\n\ndef c():\n    pass\n"
    );
}

#[test]
fn a_gap_that_bridged_two_runs_becomes_the_run_the_scope_uses() {
    let text = "fn a() {}\n\n\nfn b() {}\n\nfn c() {}\n\nfn d() {}\n";
    let case = case(
        text,
        &[("a", 1, 1), ("b", 4, 4), ("c", 6, 6), ("d", 8, 8)],
        vec![],
    );

    assert_eq!(
        case.run(&[after("b", "d")]).unwrap(),
        "fn a() {}\n\nfn c() {}\n\nfn d() {}\n\nfn b() {}\n"
    );
}
