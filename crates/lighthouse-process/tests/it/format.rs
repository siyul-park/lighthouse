use std::{collections::BTreeMap, path::Path};

use lighthouse_process::{Formatter, Output, Policy, Stdin};

fn formatter(argv: &[&str], stdin: Stdin, output: Output) -> Formatter {
    Formatter {
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        stdin,
        output,
        env: BTreeMap::new(),
    }
}

fn upper() -> Formatter {
    formatter(&["tr", "a-z", "A-Z"], Stdin::File, Output::Text)
}

#[test]
fn a_formatter_that_prints_text_returns_what_it_printed() {
    let root = tempfile::tempdir().unwrap();

    let text = upper().format(
        root.path(),
        Path::new("a.txt"),
        "ab\ncd\n",
        &Policy::default(),
    );

    assert_eq!(text.as_deref(), Ok("AB\nCD\n"));
}

#[test]
fn a_formatter_that_works_in_place_gets_a_scratch_copy_of_the_file() {
    let root = tempfile::tempdir().unwrap();
    let in_place = formatter(
        &[
            "sh",
            "-c",
            "tr a-z A-Z < \"$0\" > \"$0.new\" && mv \"$0.new\" \"$0\"",
        ],
        Stdin::None,
        Output::InPlace,
    );

    let text = in_place.format(
        root.path(),
        Path::new("src/a.txt"),
        "ab\n",
        &Policy::default(),
    );

    assert_eq!(text.as_deref(), Ok("AB\n"));
    assert!(
        !root.path().join("src/a.txt").exists(),
        "the real file is not touched"
    );
}

#[test]
fn a_formatter_that_fails_is_an_error_naming_it() {
    let root = tempfile::tempdir().unwrap();
    let failing = formatter(&["false"], Stdin::File, Output::Text);

    let text = failing.format(root.path(), Path::new("a.txt"), "ab\n", &Policy::default());

    assert!(text.unwrap_err().starts_with("formatter `false` failed"));
}

#[test]
fn a_formatter_that_prints_nothing_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let silent = formatter(&["true"], Stdin::File, Output::Text);

    let text = silent.format(root.path(), Path::new("a.txt"), "ab\n", &Policy::default());

    assert_eq!(text, Err("the formatter printed no text".to_owned()));
}

#[test]
fn a_formatter_that_prints_more_than_it_may_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let flood = formatter(
        &["sh", "-c", "yes | head -c 2000000"],
        Stdin::File,
        Output::Text,
    );

    let text = flood.format(root.path(), Path::new("a.txt"), "ab\n", &Policy::default());

    assert!(text.unwrap_err().contains("printed more than it may"));
}
