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

#[test]
fn a_reorder_that_keeps_every_line_keeps_the_declarations() {
    let text = "fn a() {}\n\nfn b() {}\n\nfn c() {}\n";
    // `c` is taken with the blank line before it and put before `a`.
    let delete = TextEdit::delete(PathBuf::new(), 21..32);
    let insert = TextEdit::insert(PathBuf::new(), 0, "fn c() {}\n\n".to_owned());
    assert_eq!(
        apply(text, &[&delete, &insert]).unwrap(),
        "fn c() {}\n\nfn a() {}\n\nfn b() {}\n"
    );
    assert_eq!(keeps_declarations(text, &[&delete, &insert]), Ok(()));
}

#[test]
fn a_move_edit_set_that_repeats_or_drops_a_declaration_is_declined() {
    let text = "fn a() {}\n\nfn b() {}\n\nfn c() {}\n";
    let copy = TextEdit::insert(PathBuf::new(), 0, "fn c() {}\n\n".to_owned());
    let why = keeps_declarations(text, &[&copy]).unwrap_err();
    assert_eq!(
        why,
        "the reorder would not keep the file's declarations: it repeats `fn c() {}` 1 time(s)"
    );
    let lost = TextEdit::delete(PathBuf::new(), 11..21);
    let why = keeps_declarations(text, &[&lost]).unwrap_err();
    assert_eq!(
        why,
        "the reorder would not keep the file's declarations: it drops `fn b() {}` 1 time(s)"
    );
}

#[test]
fn inserts_at_one_place_apply_in_the_order_of_their_sources() {
    let text = "x = 1\ny = 2\n";
    // The later source is listed first, as the order of the findings may.
    let later = TextEdit::insert_from(PathBuf::new(), 0, "b\n".to_owned(), 20);
    let earlier = TextEdit::insert_from(PathBuf::new(), 0, "a\n".to_owned(), 10);

    assert_eq!(
        apply(text, &[&later, &earlier]).unwrap(),
        "a\nb\nx = 1\ny = 2\n"
    );
}
