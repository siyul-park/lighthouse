//! The small functions of reading a SARIF log: placeholders, the identity of a
//! message and percent-escapes.

use lighthouse_checks::command::sarif::{place::decode, words::fill, words::identifying};

fn arguments(of: &[&str]) -> Vec<String> {
    of.iter().map(|a| (*a).to_owned()).collect()
}

#[test]
fn fill_replaces_each_placeholder_with_its_argument() {
    let cases = [
        ("Rename {0} to {1}", &["a", "b"][..], "Rename a to b"),
        ("{1}{0}{1}", &["x", "y"], "yxy"),
        ("no placeholder", &["a"], "no placeholder"),
        ("missing {2} stays", &["a"], "missing {2} stays"),
        ("{name} is not an index", &["a"], "{name} is not an index"),
        ("unclosed {0", &["a"], "unclosed {0"),
        ("{", &[], "{"),
        ("é{0}é", &["😀"], "é😀é"),
    ];
    for (template, args, want) in cases {
        assert_eq!(fill(template, &arguments(args)), want, "{template}");
    }
}

#[test]
fn identifying_replaces_digit_runs_and_keeps_every_other_character() {
    let cases = [
        ("line 12 of 345", "line # of #"),
        ("don't call `os.Remove` here", "don't call `os.Remove` here"),
        ("it's \"quoted\" 'text'", "it's \"quoted\" 'text'"),
        ("v1.2.3", "v#.#.#"),
        ("", ""),
        ("é9😀", "é#😀"),
    ];
    for (message, want) in cases {
        assert_eq!(identifying(message), want, "{message}");
    }
    assert_ne!(
        identifying("unchecked `os.Remove`"),
        identifying("unchecked `os.Mkdir`")
    );
}

#[test]
fn decode_reads_percent_escapes_and_leaves_what_is_not_one() {
    let cases = [
        ("a%20b", "a b"),
        ("%C3%A9", "é"),
        ("100%", "100%"),
        ("%zz", "%zz"),
        ("%2", "%2"),
        ("plain/path.go", "plain/path.go"),
    ];
    for (text, want) in cases {
        assert_eq!(decode(text), want, "{text}");
    }
}
