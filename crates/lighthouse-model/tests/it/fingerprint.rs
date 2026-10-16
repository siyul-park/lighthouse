use lighthouse_model::{Diagnostic, Fingerprint, Position, Project, Severity, Span};

#[test]
fn fingerprint_is_deterministic() {
    let a = Fingerprint::new("core/x", "pkg::f", "fn f() {}");
    assert_eq!(a, Fingerprint::new("core/x", "pkg::f", "fn f() {}"));
}

#[test]
fn fingerprint_ignores_whitespace_layout() {
    let a = Fingerprint::new("core/x", "pkg::f", "fn  f() {\n    1\n}");
    let b = Fingerprint::new("core/x", "pkg::f", "fn f() { 1 }");
    assert_eq!(a, b);
}

#[test]
fn fingerprint_separates_rule_symbol_and_snippet() {
    let base = Fingerprint::new("a", "b", "c");
    assert_ne!(base, Fingerprint::new("ab", "", "c"));
    assert_ne!(base, Fingerprint::new("a", "b", "d"));
    assert_ne!(base, Fingerprint::new("a", "c", "c"));
}

#[test]
fn fingerprint_is_hex_sha256() {
    let fp = Fingerprint::new("a", "b", "c");
    assert_eq!(fp.as_str().len(), 64);
    assert!(fp.as_str().bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(Fingerprint::from_raw(fp.as_str()), fp);
}

#[test]
fn severity_round_trips_through_text() {
    for s in [Severity::Error, Severity::Warn, Severity::Info] {
        assert_eq!(s.to_string().parse::<Severity>().unwrap(), s);
    }
    assert!("fatal".parse::<Severity>().is_err());
    assert!("review".parse::<Severity>().is_err());
}

#[test]
fn fingerprint_value_is_pinned() {
    assert_eq!(
        Fingerprint::new("core/x", "pkg::f", "fn f() {}").as_str(),
        "d63283d0f44c0f96f75a880a134f88088747876e2feea0e48f71a3895895e055"
    );
}

#[test]
fn fingerprint_normalizes_path_separators() {
    assert_eq!(
        Fingerprint::new("r", "src\\a.txt", ""),
        Fingerprint::new("r", "src/a.txt", "")
    );
}

#[test]
fn occurrence_distinguishes_repeats_and_keeps_the_first() {
    let fp = Fingerprint::new("r", "s", "x");
    assert_eq!(fp.occurrence(0), fp);
    assert_ne!(fp.occurrence(1), fp);
    assert_ne!(fp.occurrence(1), fp.occurrence(2));
}

#[test]
fn discriminate_separates_findings_by_a_stable_marker() {
    let base = Fingerprint::new("a", "b", "c");
    let one = base.discriminate("m::f#function");
    assert_ne!(one, base);
    assert_eq!(one, base.discriminate("m::f#function"));
    assert_ne!(one, base.discriminate("m::g#function"));
    assert_ne!(one.occurrence(1), one);
}

#[test]
fn findings_that_share_an_identity_are_told_apart_by_ordinal_and_reported() {
    let at = Position { line: 1, col: 1 };
    let span = Span { start: at, end: at };
    let found = |rule: &str, line: u32| {
        let mut d = Diagnostic::new(
            rule,
            Severity::Warn,
            "m",
            "a.go",
            span,
            Fingerprint::new(rule, "a.go", "x"),
        );
        d.span.start.line = line;
        d
    };
    let mut all = [found("r", 1), found("r", 2), found("other", 3)];
    let alone = all[2].fingerprint.clone();
    let first = all[0].fingerprint.clone();

    let ordinal = Diagnostic::disambiguate(&mut all, &Project::default());

    assert_eq!(all[2].fingerprint, alone);
    assert_eq!(all[0].fingerprint, first);
    assert_eq!(all[1].fingerprint, first.occurrence(1));
    assert_eq!(ordinal.len(), 2);
    assert!(ordinal.contains(&first) && ordinal.contains(&first.occurrence(1)));
}
