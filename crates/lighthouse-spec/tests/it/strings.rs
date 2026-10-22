//! The strings extension of CEL, against what cel-go's `ext.Strings` gives.

use cel::{Context, Program, Value};
use lighthouse_spec::strings;

fn eval(source: &str) -> Value {
    let mut context = Context::default();
    strings(&mut context);
    Program::compile(source)
        .unwrap()
        .execute(&context)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
}

fn pieces(source: &str) -> Vec<String> {
    let Value::List(list) = eval(source) else {
        panic!("{source} is not a list");
    };
    list.iter()
        .map(|piece| match piece {
            Value::String(text) => text.to_string(),
            other => panic!("{other:?} is not a string"),
        })
        .collect()
}

fn text(source: &str) -> String {
    match eval(source) {
        Value::String(text) => text.to_string(),
        other => panic!("{source} is {other:?}"),
    }
}

#[test]
fn strings_split_cuts_at_every_separator() {
    assert_eq!(pieces(r#""a,b,,c".split(",")"#), ["a", "b", "", "c"]);
    assert_eq!(pieces(r#""abc".split("x")"#), ["abc"]);
}

#[test]
fn strings_split_with_an_empty_separator_cuts_between_the_characters() {
    assert_eq!(pieces(r#""héy".split("")"#), ["h", "é", "y"]);
    assert_eq!(pieces(r#""".split("")"#), Vec::<String>::new());
}

#[test]
fn strings_split_limit_gives_none_for_zero_all_for_negative_and_the_rest_in_the_last() {
    assert!(pieces(r#""a,b,c".split(",", 0)"#).is_empty());
    assert_eq!(pieces(r#""a,b,c".split(",", 1)"#), ["a,b,c"]);
    assert_eq!(pieces(r#""a,b,c".split(",", 2)"#), ["a", "b,c"]);
    assert_eq!(pieces(r#""a,b,c".split(",", -1)"#), ["a", "b", "c"]);
    assert_eq!(pieces(r#""abc".split("", 2)"#), ["a", "bc"]);
}

#[test]
fn strings_replace_changes_every_occurrence_or_the_first_limit_of_them() {
    assert_eq!(text(r#""a-b-c".replace("-", "+")"#), "a+b+c");
    assert_eq!(text(r#""a-b-c".replace("-", "+", 1)"#), "a+b-c");
    assert_eq!(text(r#""a-b-c".replace("-", "+", 0)"#), "a-b-c");
    assert_eq!(text(r#""a-b-c".replace("-", "+", -1)"#), "a+b+c");
    assert_eq!(text(r#""abc".replace("", "-")"#), "-a-b-c-");
}

#[test]
fn strings_trim_drops_the_white_space_around() {
    assert_eq!(text("\"  a b \\n\".trim()"), "a b");
}

#[test]
fn strings_refuse_what_is_not_a_string_or_a_count_by_failing_the_expression() {
    let mut context = Context::default();
    strings(&mut context);
    for source in [
        r#""a".split(1)"#,
        r#""a".split(",", "x")"#,
        r#""a".replace("a")"#,
    ] {
        assert!(
            Program::compile(source).unwrap().execute(&context).is_err(),
            "{source}"
        );
    }
}
