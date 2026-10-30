//! Lowering deletes, range edits and the iota groups a delete must not renumber.

use super::*;

#[test]
fn deleting_a_spec_of_an_iota_group_keeps_the_place_of_the_specs_after_it() {
    let text = "const (\na = iota\nb\nc\n)\n";
    let case = case(text, &[("a", 2, 2), ("b", 3, 3), ("c", 4, 4)], vec![]);

    let deleted = case.run(&[EditOp::Delete { node: id("a") }]).unwrap();

    assert_eq!(deleted, "const (\n_ = iota\nb\nc\n)\n");
}

#[test]
fn deleting_a_middle_spec_of_an_iota_group_keeps_its_place_as_the_blank_identifier() {
    let text = "const (\na = iota\nb\nc\n)\n";
    let case = case(text, &[("a", 2, 2), ("b", 3, 3), ("c", 4, 4)], vec![]);

    let deleted = case.run(&[EditOp::Delete { node: id("b") }]).unwrap();

    assert_eq!(deleted, "const (\na = iota\n_\nc\n)\n");
}

#[test]
fn deleting_the_last_spec_of_an_iota_group_changes_nothing_after_it() {
    let text = "const (\na = iota\nb\nc\n)\n";
    let case = case(text, &[("a", 2, 2), ("b", 3, 3), ("c", 4, 4)], vec![]);

    let deleted = case.run(&[EditOp::Delete { node: id("c") }]).unwrap();

    assert_eq!(deleted, "const (\na = iota\nb\n)\n");
}

#[test]
fn deleting_a_spec_with_a_value_of_its_own_deletes_the_line() {
    let text = "const (\na = 1\nb = 2\nc = 3\n)\n";
    let case = case(text, &[("a", 2, 2), ("b", 3, 3), ("c", 4, 4)], vec![]);

    let deleted = case.run(&[EditOp::Delete { node: id("b") }]).unwrap();

    assert_eq!(deleted, "const (\na = 1\nc = 3\n)\n");
}

#[test]
fn delete_removes_the_block_and_one_blank_line() {
    let case = case(THREE, &[("a", 1, 1), ("b", 3, 3), ("c", 5, 5)], vec![]);

    let deleted = case.run(&[EditOp::Delete { node: id("b") }]).unwrap();

    assert_eq!(deleted, "fn a() {}\n\nfn c() {}\n");
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

#[test]
fn a_trailing_comment_does_not_make_a_spec_look_like_it_has_a_value() {
    let text = "const (\na = iota\nb // is 1 = one\nc\n)\n";
    let case = case(text, &[("a", 2, 2), ("b", 3, 3), ("c", 4, 4)], vec![]);

    let deleted = case.run(&[EditOp::Delete { node: id("a") }]).unwrap();

    assert_eq!(deleted, "const (\n_ = iota\nb // is 1 = one\nc\n)\n");
}

#[test]
fn iota_before_the_spec_does_not_keep_it_when_the_specs_after_it_give_their_own_values() {
    let text = "const (\na = iota\nb\nc = 5\nd = 6\n)\n";
    let case = case(
        text,
        &[("a", 2, 2), ("b", 3, 3), ("c", 4, 4), ("d", 5, 5)],
        vec![],
    );

    let deleted = case.run(&[EditOp::Delete { node: id("c") }]).unwrap();

    assert_eq!(deleted, "const (\na = iota\nb\nd = 6\n)\n");
}

#[test]
fn a_spec_that_repeats_the_expression_of_the_removed_one_keeps_its_place() {
    let text = "const (\na = 7\nb\nc = 9\n)\n";
    let case = case(text, &[("a", 2, 2), ("b", 3, 3), ("c", 4, 4)], vec![]);

    let deleted = case.run(&[EditOp::Delete { node: id("a") }]).unwrap();

    assert_eq!(deleted, "const (\n_ = 7\nb\nc = 9\n)\n");
}

#[test]
fn a_spec_named_by_a_later_spec_cannot_be_removed() {
    let text = "const (\na = 1\nb = a + 1\n)\n";
    let case = case(text, &[("a", 2, 2), ("b", 3, 3)], vec![]);

    let why = case.run(&[EditOp::Delete { node: id("a") }]).unwrap_err();

    assert!(why.contains("named by a spec after it"), "{why}");
}
