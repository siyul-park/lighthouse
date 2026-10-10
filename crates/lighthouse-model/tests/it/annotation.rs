use lighthouse_model::annotation::{self, ANNOTATION_REASON, Form, UNUSED_ALLOW};

#[test]
fn parse_reads_rules_and_reason_from_a_comment() {
    let allow = annotation::parse("// lighthouse:allow design/a -- because").unwrap();
    assert_eq!(allow.rules, ["design/a"]);
    assert_eq!(allow.reason.as_deref(), Some("because"));
    assert_eq!(allow.form, Form::NextLine);
    assert_eq!(allow.marker, "lighthouse:allow");

    let many = annotation::parse("/// lighthouse:allow design/a,  testing/b -- two rules").unwrap();
    assert_eq!(many.rules, ["design/a", "testing/b"]);

    let block = annotation::parse("/* lighthouse:allow design/a -- in a block */").unwrap();
    assert_eq!(block.reason.as_deref(), Some("in a block"));

    let later = annotation::parse("// Why:\n// lighthouse:allow design/a -- second line").unwrap();
    assert_eq!(later.rules, ["design/a"]);
}

#[test]
fn parse_reads_the_eslint_forms() {
    let forms = [
        ("lighthouse-disable", Form::Disable),
        ("lighthouse-disable-next-line", Form::NextLine),
        ("lighthouse-disable-line", Form::Line),
    ];
    for (marker, form) in forms {
        let directive =
            annotation::parse(&format!("// {marker} design/a, testing/b -- why")).unwrap();
        assert_eq!(directive.form, form, "{marker}");
        assert_eq!(directive.marker, marker);
        assert_eq!(directive.rules, ["design/a", "testing/b"]);
        assert_eq!(directive.reason.as_deref(), Some("why"));
        assert!(directive.form.needs_reason());
    }
    let enable = annotation::parse("# lighthouse-enable design/a").unwrap();
    assert_eq!(enable.form, Form::Enable);
    assert_eq!(enable.rules, ["design/a"]);
    assert!(!enable.form.needs_reason());

    let block = annotation::parse("/* lighthouse-disable-line design/a -- here */").unwrap();
    assert_eq!(block.reason.as_deref(), Some("here"));
}

#[test]
fn a_directive_names_rules_there_is_no_form_without_ids() {
    let blanket = annotation::parse("// lighthouse-disable -- all of them").unwrap();
    assert!(blanket.rules.is_empty());
    assert_eq!(blanket.reason.as_deref(), Some("all of them"));
    assert!(
        annotation::parse("// lighthouse-enable")
            .unwrap()
            .rules
            .is_empty()
    );
    assert!(
        annotation::parse("// lighthouse:allow")
            .unwrap()
            .rules
            .is_empty()
    );
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
    assert!(annotation::parse("// see lighthouse:allow design/a -- no").is_none());
    assert!(annotation::parse("// see lighthouse-disable design/a -- no").is_none());
    assert!(annotation::parse("// lighthouse-disables design/a -- no").is_none());
    assert!(annotation::parse("// lighthouse-disable-next-liner design/a -- no").is_none());
    assert!(annotation::parse("// just a comment").is_none());
    assert_eq!(ANNOTATION_REASON, "core/annotation-reason");
    assert_eq!(UNUSED_ALLOW, "core/unused-allow");
}

#[test]
fn directives_lists_every_line_that_holds_one() {
    let text = "// Why:\n// lighthouse-disable design/a -- one\n// lighthouse-enable design/a\n// and so on";
    let found = annotation::directives(text);
    assert_eq!(
        found
            .iter()
            .map(|(at, d)| (*at, d.form))
            .collect::<Vec<_>>(),
        [(1, Form::Disable), (2, Form::Enable)]
    );
    assert!(annotation::directives("// nothing").is_empty());
}

#[test]
fn line_of_finds_the_line_that_holds_the_annotation() {
    assert_eq!(
        annotation::line_of("// lighthouse:allow design/a -- why"),
        Some(0)
    );
    assert_eq!(
        annotation::line_of("// Why:\n// lighthouse:allow design/a -- second line"),
        Some(1)
    );
    assert_eq!(annotation::line_of("// just a comment"), None);
}
