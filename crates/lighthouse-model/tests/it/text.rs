use lighthouse_model::{LineIndex, Position, Span};

fn at(line: u32, col: u32) -> Position {
    Position { line, col }
}

const TEXT: &str = "ab\ncde\n\nf";

#[test]
fn offsets_and_positions_are_inverse() {
    let index = LineIndex::new(TEXT);

    assert_eq!(index.offset(at(1, 1)), Some(0));
    assert_eq!(index.offset(at(2, 3)), Some(5));
    assert_eq!(index.offset(at(4, 2)), Some(TEXT.len()));
    for offset in 0..=TEXT.len() {
        assert_eq!(
            index.offset(index.position(offset)),
            Some(offset),
            "{offset}"
        );
    }
    assert_eq!(index.position(3), at(2, 1));
}

#[test]
fn positions_outside_the_text_have_no_offset() {
    let index = LineIndex::new(TEXT);

    assert_eq!(index.offset(at(0, 1)), None);
    assert_eq!(index.offset(at(1, 0)), None);
    assert_eq!(index.offset(at(1, 9)), None);
    assert_eq!(index.offset(at(9, 1)), None);
}

#[test]
fn the_line_after_the_last_newline_is_the_end_of_the_text() {
    let index = LineIndex::new("a\n");

    assert_eq!(index.offset(at(2, 1)), Some(2));
    assert_eq!(LineIndex::new("a").offset(at(2, 1)), Some(1));
}

#[test]
fn a_span_is_a_byte_range_and_columns_count_bytes() {
    let text = "é = 1\n";
    let index = LineIndex::new(text);

    let range = index
        .range(Span {
            start: at(1, 4),
            end: at(1, 5),
        })
        .unwrap();

    assert_eq!(&text[range], "=");
    assert_eq!(
        index.range(Span {
            start: at(1, 2),
            end: at(1, 3)
        }),
        None,
        "inside a multi-byte character"
    );
}

#[test]
fn line_start_and_end_bound_the_line_holding_an_offset() {
    let index = LineIndex::new(TEXT);

    assert_eq!(index.line_start(4), 3);
    assert_eq!(index.line_end(4), 7);
    assert_eq!(index.line_start(8), 8);
    assert_eq!(index.line_end(8), TEXT.len());
}
