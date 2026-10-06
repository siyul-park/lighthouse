use lighthouse_model::annotation::{self, ANNOTATION_REASON, UNUSED_ALLOW};

#[test]
fn parse_reads_rules_and_reason_from_a_comment() {
    let allow = annotation::parse("// lighthouse:allow design/a -- because").unwrap();
    assert_eq!(allow.rules, ["design/a"]);
    assert_eq!(allow.reason.as_deref(), Some("because"));

    let many = annotation::parse("/// lighthouse:allow design/a,  testing/b -- two rules").unwrap();
    assert_eq!(many.rules, ["design/a", "testing/b"]);

    let block = annotation::parse("/* lighthouse:allow design/a -- in a block */").unwrap();
    assert_eq!(block.reason.as_deref(), Some("in a block"));

    let later = annotation::parse("// Why:\n// lighthouse:allow design/a -- second line").unwrap();
    assert_eq!(later.rules, ["design/a"]);
}

#[test]
fn parse_reports_a_missing_reason_and_ignores_prose() {
    assert_eq!(
        annotation::parse("// lighthouse:allow design/a")
            .unwrap()
            .reason,
        None
    );
    assert_eq!(
        annotation::parse("// lighthouse:allow design/a --  ")
            .unwrap()
            .reason,
        None
    );
    assert!(annotation::parse("// lighthouse:allow").is_none());
    assert!(annotation::parse("// see lighthouse:allow design/a -- no").is_none());
    assert!(annotation::parse("// just a comment").is_none());
    assert_eq!(ANNOTATION_REASON, "core/annotation-reason");
    assert_eq!(UNUSED_ALLOW, "core/unused-allow");
}
