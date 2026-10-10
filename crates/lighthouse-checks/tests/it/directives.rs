//! The suppression directives in their ESLint forms, through the engine and
//! the Rust provider: `lighthouse-disable` with `lighthouse-enable`,
//! `lighthouse-disable-next-line` and `lighthouse-disable-line`.

use lighthouse_engine::Outcome;

use crate::annotations::{DOC_RULE, check, rules_of};

const BANNERS: &str = "\"design/no-banners\" = \"warn\"\n";

fn lines_of(outcome: &Outcome, rule: &str) -> Vec<u32> {
    outcome
        .diagnostics
        .iter()
        .filter(|d| d.rule_id == rule)
        .map(|d| d.span.start.line)
        .collect()
}

fn message_of<'a>(outcome: &'a Outcome, rule: &str) -> &'a str {
    outcome
        .diagnostics
        .iter()
        .find(|d| d.rule_id == rule)
        .map_or("", |d| d.message.as_str())
}

#[test]
fn disable_next_line_covers_the_next_line_and_the_symbol_declared_there() {
    let outcome = check(
        "// lighthouse-disable-next-line design/exported-doc -- documented at its origin\npub fn open() {}\n\npub fn close() {}\n",
        DOC_RULE,
    );

    assert_eq!(rules_of(&outcome), ["design/exported-doc"]);
    assert_eq!(lines_of(&outcome, "design/exported-doc"), [4]);
    assert_eq!(outcome.allowed.len(), 1);
    assert_eq!(outcome.allowed[0].reason, "documented at its origin");
}

#[test]
fn disable_line_covers_its_own_line_only() {
    let outcome = check(
        "pub fn open() {} // lighthouse-disable-line design/exported-doc -- shim\npub fn close() {}\n",
        DOC_RULE,
    );

    assert_eq!(lines_of(&outcome, "design/exported-doc"), [2]);
    assert_eq!(outcome.allowed.len(), 1);
    assert!(!rules_of(&outcome).contains(&"core/no-unused-allow"));
}

#[test]
fn disable_runs_to_the_matching_enable() {
    let outcome = check(
        "pub fn first() {}\n// lighthouse-disable design/exported-doc -- generated block\npub fn second() {}\n\npub fn third() {}\n// lighthouse-enable design/exported-doc\npub fn fourth() {}\n",
        DOC_RULE,
    );

    assert_eq!(lines_of(&outcome, "design/exported-doc"), [1, 7]);
    assert_eq!(outcome.allowed.len(), 2);
    assert_eq!(
        rules_of(&outcome),
        ["design/exported-doc", "design/exported-doc"]
    );
}

#[test]
fn disable_without_an_enable_runs_to_the_end_of_the_file() {
    let outcome = check(
        "pub fn first() {}\n// lighthouse-disable design/exported-doc -- the rest is generated\npub fn second() {}\npub fn third() {}\n",
        DOC_RULE,
    );

    assert_eq!(lines_of(&outcome, "design/exported-doc"), [1]);
    assert_eq!(outcome.allowed.len(), 2);
}

#[test]
fn disable_at_the_top_of_a_file_waives_the_whole_file() {
    let outcome = check(
        "// lighthouse-disable design/exported-doc -- a generated file\npub fn first() {}\npub fn second() {}\n",
        DOC_RULE,
    );

    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    assert_eq!(outcome.allowed.len(), 2);
}

#[test]
fn ranges_of_different_rules_nest() {
    let source = "pub fn a() {}\n\
        // lighthouse-disable design/exported-doc -- outer\n\
        pub fn b() {\n    \
            // lighthouse-disable design/no-banners -- inner\n    \
            let _ = 1;\n    \
            // ======== helpers ========\n    \
            let _ = 2;\n    \
            // lighthouse-enable design/no-banners\n    \
            let _ = 3;\n    \
            // ======== after ========\n    \
            let _ = 4;\n\
        }\n\
        // lighthouse-enable design/exported-doc\n\
        pub fn c() {}\n";
    let outcome = check(source, &format!("{DOC_RULE}{BANNERS}"));

    assert_eq!(lines_of(&outcome, "design/exported-doc"), [1, 14]);
    assert_eq!(lines_of(&outcome, "design/no-banners"), [10]);
    assert_eq!(outcome.diagnostics.len(), 3, "{:?}", outcome.diagnostics);
    assert_eq!(outcome.allowed.len(), 2);
}

#[test]
fn an_enable_that_closes_nothing_is_reported() {
    let outcome = check(
        "// lighthouse-enable design/exported-doc\n/// Documented.\npub fn open() {}\n",
        DOC_RULE,
    );

    assert_eq!(rules_of(&outcome), ["core/no-unused-allow"]);
    assert!(
        message_of(&outcome, "core/no-unused-allow").contains("no matching `lighthouse-disable`"),
        "{:?}",
        outcome.diagnostics
    );
}

#[test]
fn a_range_that_suppresses_nothing_is_reported_as_unused() {
    let outcome = check(
        "pub fn first() {}\n// lighthouse-disable design/exported-doc -- nothing below\n/// Documented.\npub fn second() {}\n",
        DOC_RULE,
    );

    assert_eq!(
        rules_of(&outcome),
        ["design/exported-doc", "core/no-unused-allow"]
    );
    assert!(
        message_of(&outcome, "core/no-unused-allow").contains("in its range"),
        "{:?}",
        outcome.diagnostics
    );
    assert!(outcome.allowed.is_empty());
}

#[test]
fn a_second_disable_inside_an_open_range_is_unused() {
    let outcome = check(
        "// lighthouse-disable design/exported-doc -- the file\n// lighthouse-disable design/exported-doc -- again\npub fn open() {}\n",
        DOC_RULE,
    );

    assert_eq!(rules_of(&outcome), ["core/no-unused-allow"]);
    assert_eq!(outcome.diagnostics[0].span.start.line, 2, "the second one");
    assert_eq!(outcome.allowed.len(), 1);
}

#[test]
fn every_disable_form_needs_a_reason() {
    for form in [
        "lighthouse-disable",
        "lighthouse-disable-next-line",
        "lighthouse-disable-line",
    ] {
        let outcome = check(
            &format!("// {form} design/exported-doc\npub fn open() {{}}\n"),
            DOC_RULE,
        );

        assert_eq!(
            rules_of(&outcome),
            ["core/allow-reason", "design/exported-doc"],
            "{form}"
        );
        assert!(outcome.allowed.is_empty(), "{form}");
        assert!(
            message_of(&outcome, "core/allow-reason").contains(form),
            "{form}"
        );
    }
}

#[test]
fn there_is_no_form_without_ids() {
    let outcome = check(
        "// lighthouse-disable -- everything\npub fn open() {}\n",
        DOC_RULE,
    );

    assert_eq!(
        rules_of(&outcome),
        ["core/no-unused-allow", "design/exported-doc"]
    );
    assert!(message_of(&outcome, "core/no-unused-allow").contains("names no decision"));
}

#[test]
fn two_directives_in_one_comment_each_count() {
    let outcome = check(
        "// lighthouse-disable-next-line design/exported-doc -- one\n// lighthouse-disable-next-line design/no-banners -- two\npub fn open() {}\n",
        &format!("{DOC_RULE}{BANNERS}"),
    );

    assert_eq!(rules_of(&outcome), ["core/no-unused-allow"]);
    assert!(message_of(&outcome, "core/no-unused-allow").contains("design/no-banners"));
    assert_eq!(outcome.diagnostics[0].span.start.line, 2);
    assert_eq!(outcome.allowed.len(), 1);
}
