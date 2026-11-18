use lighthouse_model::{Diagnostic, Fingerprint, Position, Severity, Span, kept};

fn finding(subject: Option<&str>) -> Diagnostic {
    let at = Position { line: 1, col: 1 };
    let mut found = Diagnostic::new(
        "p/a",
        Severity::Error,
        "m",
        "a.go",
        Span { start: at, end: at },
        Fingerprint::from_raw("f"),
    );
    found.subject = subject.map(str::to_owned);
    found
}

#[test]
fn kept_findings_read_back_with_their_subject() {
    let all = vec![finding(Some("a.go\0key")), finding(None)];

    let text = kept::to_string(&all).unwrap();

    assert_eq!(kept::from_str(&text).unwrap(), all);
}

#[test]
fn a_finding_without_a_subject_is_kept_as_it_is_reported() {
    let plain = finding(None);

    assert_eq!(
        kept::to_string(std::slice::from_ref(&plain)).unwrap(),
        serde_json::to_string(&[&plain]).unwrap()
    );
}

#[test]
fn the_reported_form_of_a_finding_has_no_subject() {
    let json = serde_json::to_string(&finding(Some("s"))).unwrap();

    assert!(!json.contains("subject"), "{json}");
    assert_eq!(
        serde_json::from_str::<Diagnostic>(&json).unwrap(),
        finding(None)
    );
}

#[test]
fn kept_serialize() {
    let mut text = Vec::new();

    kept::serialize(
        &[finding(Some("s"))],
        &mut serde_json::Serializer::new(&mut text),
    )
    .unwrap();

    let text = String::from_utf8(text).unwrap();
    assert!(text.contains(r#""subject":"s""#), "{text}");
}

#[test]
fn kept_deserialize() {
    let all = vec![finding(Some("s")), finding(None)];
    let text = kept::to_string(&all).unwrap();

    let read = kept::deserialize(&mut serde_json::Deserializer::from_str(&text)).unwrap();

    assert_eq!(read, all);
}
