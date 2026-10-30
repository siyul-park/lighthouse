use super::*;
use crate::fixture::file;

#[test]
fn a_training_file_gives_lines_and_the_examples_of_the_question() {
    let text = file(&[
        ("fn a() {}", true, "a.go", r#"{"type":"Person"}"#),
        (
            "fn b() {}",
            false,
            "a.go",
            r#"{"type":"SoftwareAgent","hadRole":"model"}"#,
        ),
    ]);
    let dataset = parse(&text).unwrap();

    assert_eq!(dataset.decision, "d/x");
    assert_eq!(dataset.invalid, ["bad", "worse"]);
    assert_eq!(dataset.valid, ["good"]);
    let labels: Vec<f64> = dataset.lines.iter().map(|l| l.label).collect();
    assert_eq!(labels, [1.0, 0.0]);
    assert_eq!(dataset.lines[0].weight, 0.7);
    assert_eq!(dataset.lines[0].group, "a.go");
    assert_eq!(sources(&dataset.lines)["model"], 1);
    assert_eq!(sources(&dataset.lines)["person"], 1);
}

#[test]
fn an_empty_or_broken_file_says_why() {
    assert!(parse("").unwrap_err().contains("no line"));
    assert!(parse("{").unwrap_err().contains("line 1"));
}
