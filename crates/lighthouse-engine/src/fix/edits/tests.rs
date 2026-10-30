use super::*;

#[test]
fn overlap_treats_inserts_at_one_point_as_compatible() {
    assert!(!overlap(&(3..3), &(3..3)));
    assert!(!overlap(&(3..3), &(3..5)));
    assert!(overlap(&(4..4), &(3..5)));
    assert!(overlap(&(1..4), &(3..5)));
    assert!(!overlap(&(1..3), &(3..5)));
}

#[test]
fn replace_changed_takes_the_lines_between_the_common_ends() {
    let edit = replace_changed(Path::new("f"), "a\nb\nc\nd\n", "a\nB\nX\nd\n").unwrap();
    assert_eq!((edit.range, edit.text.as_str()), (2..6, "B\nX\n"));
    assert!(replace_changed(Path::new("f"), "a\n", "a\n").is_none());
    let grown = replace_changed(Path::new("f"), "a\na\n", "a\na\na\n").unwrap();
    assert_eq!(apply("a\na\n", &[&grown]).unwrap(), "a\na\na\n");
}

#[test]
fn apply_rebuilds_the_text_in_one_pass() {
    let delete = TextEdit::delete(PathBuf::new(), 2..4);
    let insert = TextEdit::insert(PathBuf::new(), 6, "X".to_owned());
    assert_eq!(apply("abcdefgh", &[&insert, &delete]).unwrap(), "abefXgh");
}

#[test]
fn whole_lines_take_a_following_blank_line() {
    let text = "a\n\nb\n\nc\n";
    assert_eq!(whole_lines(text, &(3..4)), Some(3..6));
    assert_eq!(whole_lines(text, &(0..1)), Some(0..3));
}

#[test]
fn whole_lines_fall_back_to_the_preceding_blank_line() {
    let text = "a\n\nb\n";
    assert_eq!(whole_lines(text, &(3..4)), Some(2..5));
}

#[test]
fn a_preceding_blank_line_stays_when_code_follows() {
    let text = "a\n\nb\nc\n";
    assert_eq!(whole_lines(text, &(3..4)), Some(3..5));
}

#[test]
fn a_preceding_blank_line_goes_when_a_bracket_follows() {
    let text = "{\n\nb\n}\n";
    assert_eq!(whole_lines(text, &(3..4)), Some(2..5));
}

#[test]
fn a_range_that_shares_its_line_is_not_whole_lines() {
    assert_eq!(whole_lines("x := 1 // c\n", &(7..11)), None);
}
