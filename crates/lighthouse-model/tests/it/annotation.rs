use lighthouse_model::annotation::{self, ANNOTATION_REASON, Directive, Form, UNUSED_ALLOW};

fn parse(comment: &str) -> Option<Directive> {
    annotation::directives(comment)
        .into_iter()
        .next()
        .map(|(_, directive)| directive)
}

#[test]
fn parse_reads_rules_and_reason_from_a_comment() {
    let allow = parse("// lighthouse-disable-next-line design/a -- because").unwrap();
    assert_eq!(allow.rules, ["design/a"]);
    assert_eq!(allow.reason.as_deref(), Some("because"));
    assert_eq!(allow.form, Form::NextLine);
    assert_eq!(allow.marker, "lighthouse-disable-next-line");

    let many = parse("// lighthouse-disable-next-line design/a,  testing/b -- two rules").unwrap();
    assert_eq!(many.rules, ["design/a", "testing/b"]);

    let block = parse("/* lighthouse-disable-next-line design/a -- in a block */").unwrap();
    assert_eq!(block.reason.as_deref(), Some("in a block"));

    let later = parse("// Why:\n// lighthouse-disable-next-line design/a -- second line").unwrap();
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
        let directive = parse(&format!("// {marker} design/a, testing/b -- why")).unwrap();
        assert_eq!(directive.form, form, "{marker}");
        assert_eq!(directive.marker, marker);
        assert_eq!(directive.rules, ["design/a", "testing/b"]);
        assert_eq!(directive.reason.as_deref(), Some("why"));
        assert!(directive.form.needs_reason());
    }
    let enable = parse("# lighthouse-enable design/a").unwrap();
    assert_eq!(enable.form, Form::Enable);
    assert_eq!(enable.rules, ["design/a"]);
    assert!(!enable.form.needs_reason());

    let block = parse("/* lighthouse-disable-line design/a -- here */").unwrap();
    assert_eq!(block.reason.as_deref(), Some("here"));
}

#[test]
fn a_directive_names_rules_there_is_no_form_without_ids() {
    let blanket = parse("// lighthouse-disable -- all of them").unwrap();
    assert!(blanket.rules.is_empty());
    assert_eq!(blanket.reason.as_deref(), Some("all of them"));
    assert!(parse("// lighthouse-enable").unwrap().rules.is_empty());
    assert!(
        parse("// lighthouse-disable-next-line")
            .unwrap()
            .rules
            .is_empty()
    );
}

#[test]
fn parse_reports_a_missing_reason_and_ignores_prose() {
    assert_eq!(
        parse("// lighthouse-disable-next-line design/a")
            .unwrap()
            .reason,
        None
    );
    assert_eq!(
        parse("// lighthouse-disable-next-line design/a --  ")
            .unwrap()
            .reason,
        None
    );
    assert!(parse("// see lighthouse-disable-next-line design/a -- no").is_none());
    assert!(parse("// see lighthouse-disable design/a -- no").is_none());
    assert!(parse("// lighthouse-disables design/a -- no").is_none());
    assert!(parse("// lighthouse-disable-next-liner design/a -- no").is_none());
    assert!(parse("// just a comment").is_none());
    assert_eq!(ANNOTATION_REASON, "core/allow-reason");
    assert_eq!(UNUSED_ALLOW, "core/no-unused-allow");
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
fn a_documentation_comment_never_holds_a_directive() {
    for doc in [
        "/// lighthouse-disable design/a -- why",
        "//! lighthouse-disable design/a -- why",
        "/** lighthouse-disable design/a -- why */",
        "/*! lighthouse-disable design/a -- why */",
        "/**\n * lighthouse-disable design/a -- why\n */",
    ] {
        assert!(parse(doc).is_none(), "{doc}");
        assert!(annotation::directives(doc).is_empty(), "{doc}");
    }
    assert!(parse("//// lighthouse-disable design/a -- why").is_some());
    assert!(parse("/**/ lighthouse-disable design/a -- why").is_some());
    assert!(parse("/* lighthouse-disable design/a -- why */").is_some());
}

#[test]
fn form_names_are_the_eslint_spellings() {
    let names: Vec<_> = [Form::Disable, Form::Enable, Form::NextLine, Form::Line]
        .iter()
        .map(|f| f.as_str())
        .collect();
    assert_eq!(
        names,
        ["disable", "enable", "disable-next-line", "disable-line"]
    );
}
