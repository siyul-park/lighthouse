use lighthouse_model::{Diagnostic, Fingerprint, Position, Severity, Span};
use lighthouse_report::{Format, render};
use serde_json::json;

fn span(line: u32, col: u32, end_line: u32) -> Span {
    Span {
        start: Position { line, col },
        end: Position {
            line: end_line,
            col: 1,
        },
    }
}

fn fixture() -> Vec<Diagnostic> {
    let mut first = Diagnostic::new(
        "core/max-file-lines",
        Severity::Warn,
        "file has 12 lines, limit is 10",
        "src/a.txt",
        span(11, 1, 12),
        Fingerprint::of("core/max-file-lines", "src/a.txt", ""),
    );
    first.evidence = json!({ "lines": 12, "max": 10 });
    first.fix = Some("split the file".to_owned());
    let second = Diagnostic::new(
        "demo/boom",
        Severity::Error,
        "boom",
        "src/b c/é#1.txt",
        span(2, 5, 2),
        Fingerprint::of("demo/boom", "b", "boom"),
    );
    vec![first, second]
}

#[test]
fn text_is_gofmt_like() {
    insta::assert_snapshot!(render(Format::Text, &fixture()));
}

#[test]
fn json_is_one_object_per_line() {
    let out = render(Format::Json, &fixture());
    assert_eq!(out.lines().count(), 2);
    insta::assert_snapshot!(out);
}

#[test]
fn sarif_carries_partial_fingerprints() {
    insta::assert_snapshot!(render(Format::Sarif, &fixture()));
}

#[test]
fn empty_input_renders_valid_documents() {
    assert_eq!(render(Format::Text, &[]), "");
    assert_eq!(render(Format::Json, &[]), "");
    let sarif: serde_json::Value = serde_json::from_str(&render(Format::Sarif, &[])).unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    assert_eq!(sarif["runs"][0]["results"], json!([]));
}

#[test]
fn format_parses_from_text() {
    assert_eq!("sarif".parse::<Format>().unwrap(), Format::Sarif);
    assert!("xml".parse::<Format>().is_err());
}
