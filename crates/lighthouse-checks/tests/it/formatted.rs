//! The `core/formatted` check through the engine, with formatters that are
//! plain commands: `tr` upper-cases text, `false` fails.

use std::fs;

use lighthouse_engine::{Engine, Outcome};
use lighthouse_spec::{Catalog, Config};

const UPPERCASE: &str = r#"["tr", "a-z", "A-Z"]"#;

#[test]
fn a_file_equal_to_the_formatter_output_has_no_finding() {
    let outcome = check("AB\nCD\n", UPPERCASE, true);

    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    assert!(outcome.incomplete.is_empty(), "{:?}", outcome.incomplete);
}

#[test]
fn a_file_that_differs_is_one_finding_at_the_first_differing_line() {
    let outcome = check("AB\ncd\nef\n", UPPERCASE, true);

    assert_eq!(outcome.diagnostics.len(), 1, "{:?}", outcome.diagnostics);
    let found = &outcome.diagnostics[0];
    assert_eq!(found.rule_id, "core/formatted");
    assert_eq!(found.span.start.line, 2);
    assert_eq!(
        found.message,
        "a.txt differs from tr output from line 2; run the formatter"
    );
    assert_eq!(found.evidence["formatter"], "tr");
    assert_eq!(found.evidence["line"], 2);
}

#[test]
fn a_formatter_that_fails_leaves_the_file_incomplete_not_clean() {
    let outcome = check("AB\n", r#"["false"]"#, true);

    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    assert_eq!(outcome.incomplete.len(), 1, "{:?}", outcome.incomplete);
}

#[test]
fn a_project_that_is_not_trusted_skips_the_formatter_with_a_notice() {
    let outcome = check("ab\n", UPPERCASE, false);

    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    assert!(outcome.incomplete.is_empty(), "{:?}", outcome.incomplete);
    assert_eq!(outcome.notices.len(), 1, "{:?}", outcome.notices);
}

/// Checks `a.txt` holding `text` with `core/formatted`, the text language
/// formatted by `argv`.
fn check(text: &str, argv: &str, trusted: bool) -> Outcome {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), text).unwrap();
    let config = Config::parse_inline(&format!(
        "plugins = [\"core\"]\n[rules]\n\"core/formatted\" = \"warn\"\n\
         [languages.text]\nformatter = {{ argv = {argv}, stdin = \"file\", output = \"text\" }}\n"
    ))
    .unwrap();
    Engine::new(
        lighthouse_checks::registry(),
        config,
        Catalog::bundled(),
        dir.path(),
    )
    .unwrap()
    .with_trust(trusted)
    .check(&[], &[])
    .unwrap()
}
