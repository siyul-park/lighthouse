use lighthouse_model::{Fingerprint, Severity};

#[test]
fn fingerprint_is_deterministic() {
    let a = Fingerprint::of("core/x", "pkg::f", "fn f() {}");
    assert_eq!(a, Fingerprint::of("core/x", "pkg::f", "fn f() {}"));
}

#[test]
fn fingerprint_ignores_whitespace_layout() {
    let a = Fingerprint::of("core/x", "pkg::f", "fn  f() {\n    1\n}");
    let b = Fingerprint::of("core/x", "pkg::f", "fn f() { 1 }");
    assert_eq!(a, b);
}

#[test]
fn fingerprint_separates_rule_symbol_and_snippet() {
    let base = Fingerprint::of("a", "b", "c");
    assert_ne!(base, Fingerprint::of("ab", "", "c"));
    assert_ne!(base, Fingerprint::of("a", "b", "d"));
    assert_ne!(base, Fingerprint::of("a", "c", "c"));
}

#[test]
fn fingerprint_is_hex_sha256() {
    let fp = Fingerprint::of("a", "b", "c");
    assert_eq!(fp.as_str().len(), 64);
    assert!(fp.as_str().bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn severity_round_trips_through_text() {
    for s in [
        Severity::Error,
        Severity::Warn,
        Severity::Review,
        Severity::Info,
    ] {
        assert_eq!(s.to_string().parse::<Severity>().unwrap(), s);
    }
    assert!("fatal".parse::<Severity>().is_err());
}

#[test]
fn fingerprint_value_is_pinned() {
    assert_eq!(
        Fingerprint::of("core/x", "pkg::f", "fn f() {}").as_str(),
        "d63283d0f44c0f96f75a880a134f88088747876e2feea0e48f71a3895895e055"
    );
}

#[test]
fn fingerprint_normalizes_path_separators() {
    assert_eq!(
        Fingerprint::of("r", "src\\a.txt", ""),
        Fingerprint::of("r", "src/a.txt", "")
    );
}

#[test]
fn occurrence_distinguishes_repeats_and_keeps_the_first() {
    let fp = Fingerprint::of("r", "s", "x");
    assert_eq!(fp.occurrence(0), fp);
    assert_ne!(fp.occurrence(1), fp);
    assert_ne!(fp.occurrence(1), fp.occurrence(2));
}
