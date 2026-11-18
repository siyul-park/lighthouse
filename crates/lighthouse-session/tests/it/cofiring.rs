//! The co-firing of decisions over planted findings: a decision whose
//! subjects another reports on entirely, not at all, and at the edges of the
//! thresholds.

use std::{collections::BTreeMap, path::Path};

use lighthouse_model::{Diagnostic, Fingerprint, Position, Severity, Span};
use lighthouse_session::{Cofire, MIN_SHARE, MIN_SUBJECTS, cofiring, likely_implied_by};
use lighthouse_spec::Catalog;

use crate::fixture::{LONG, project, session, write};

/// A decision about `subject`, which selects `select` (the subject itself
/// when it is empty).
fn decision(name: &str, subject: &str, select: &str, languages: &str) -> String {
    let select = if select.is_empty() { subject } else { select };
    format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/{name}\nspec:\n  title: T\n  context: C\n  scope: {{ subject: {subject} }}\n  requirement: R MUST hold.\n  severity: warn\n  check:\n    type: cel\n    select: {select}\n    where: 'true'\n    message: m\n{languages}  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{{ path: a.txt, body: x }}]\n    - name: good\n      language: text\n      kind: valid\n      files: [{{ path: a.txt, body: x }}]\n"
    )
}

fn catalog() -> Catalog {
    let files = BTreeMap::from([
        ("a.yaml".to_owned(), decision("a", "symbol", "", "")),
        ("b.yaml".to_owned(), decision("b", "symbol", "", "")),
        ("c.yaml".to_owned(), decision("c", "file", "", "")),
        (
            "k1.yaml".to_owned(),
            decision("k1", "symbol", "comment", ""),
        ),
        (
            "k2.yaml".to_owned(),
            decision("k2", "symbol", "comment", ""),
        ),
        (
            "go.yaml".to_owned(),
            decision("go", "symbol", "", "  languages:\n    go: {}\n"),
        ),
        (
            "rs.yaml".to_owned(),
            decision("rs", "symbol", "", "  languages:\n    rust: {}\n"),
        ),
    ]);
    Catalog::from_local(files, &lighthouse_test_support::cel()).unwrap()
}

/// The language of a file by its extension.
fn language(file: &Path) -> Option<String> {
    match file.extension()?.to_str()? {
        "go" => Some("go".to_owned()),
        "rs" => Some("rust".to_owned()),
        _ => None,
    }
}

/// A finding of `rule` about the value `subject` of `file`.
fn finding(rule: &str, file: &str, subject: usize) -> Diagnostic {
    let at = Position { line: 1, col: 1 };
    let mut found = Diagnostic::new(
        format!("local/{rule}"),
        Severity::Warn,
        "m",
        file,
        Span { start: at, end: at },
        Fingerprint::new(rule, &subject.to_string(), ""),
    );
    found.subject = Some(format!("{file}\0{subject}"));
    found
}

fn findings(rule: &str, file: &str, subjects: impl IntoIterator<Item = usize>) -> Vec<Diagnostic> {
    subjects
        .into_iter()
        .map(|s| finding(rule, file, s))
        .collect()
}

fn between<'a>(all: &'a [Cofire], decision: &str, by: &str) -> Option<&'a Cofire> {
    all.iter()
        .find(|c| c.decision == format!("local/{decision}") && c.by == format!("local/{by}"))
}

fn cofire(subjects: usize, shared: usize) -> Cofire {
    Cofire {
        language: "go".to_owned(),
        decision: "n".to_owned(),
        by: "s".to_owned(),
        subjects,
        shared,
    }
}

#[test]
fn cofire_percent() {
    assert!((cofire(20, 18).percent() - 90.0).abs() < f64::EPSILON);
    assert!(cofire(0, 0).percent().abs() < f64::EPSILON);
}

#[test]
fn cofire_likely() {
    assert!(cofire(MIN_SUBJECTS, MIN_SUBJECTS).likely());
    assert!(cofire(20, 18).likely(), "90% of 20 is the edge");
    assert!(!cofire(100, 89).likely(), "89% is not enough");
    assert!(
        !cofire(MIN_SUBJECTS - 1, MIN_SUBJECTS - 1).likely(),
        "19 subjects are too few"
    );
    assert_eq!(MIN_SHARE, 90);
}

#[test]
fn cofiring_full_and_none() {
    let mut all = findings("a", "x.go", 0..30);
    all.extend(findings("b", "x.go", 0..30));
    all.extend(findings("c", "x.go", 0..30));
    all.extend(findings("go", "x.go", 100..130));

    let told = cofiring(&catalog(), &all, &language);

    let full = between(&told, "a", "b").unwrap();
    assert_eq!(
        (full.subjects, full.shared, full.language.as_str()),
        (30, 30, "go")
    );
    assert!(full.likely());
    let none = between(&told, "a", "go").unwrap();
    assert_eq!((none.subjects, none.shared), (30, 0));
    assert!(!none.likely());
    assert!(
        between(&told, "a", "c").is_none(),
        "another sort is not compared"
    );
    assert!(
        between(&told, "a", "a").is_none(),
        "a decision is not compared with itself"
    );
}

#[test]
fn cofiring_compares_decisions_of_a_language_both_apply_to() {
    let mut all = findings("go", "x.go", 0..30);
    all.extend(findings("rs", "x.go", 0..30));
    all.extend(findings("a", "x.go", 0..30));

    let told = cofiring(&catalog(), &all, &language);

    assert!(
        between(&told, "go", "rs").is_none(),
        "no language in common"
    );
    assert!(
        between(&told, "go", "a").is_some(),
        "a decision of every language meets both"
    );
}

#[test]
fn cofiring_keys_a_subject_by_what_the_rule_selected_not_by_the_file() {
    let mut all: Vec<Diagnostic> = (0..30).map(|c| finding("k1", "x.go", c)).collect();
    all.extend((0..30).map(|c| finding("k2", "x.go", 100 + c)));

    let told = cofiring(&catalog(), &all, &language);

    let apart = between(&told, "k1", "k2").unwrap();
    assert_eq!(
        (apart.subjects, apart.shared),
        (30, 0),
        "other comments of the same file"
    );
}

#[test]
fn cofiring_is_told_per_language() {
    let mut all = findings("a", "x.go", 0..30);
    all.extend(findings("a", "y.rs", 0..30));
    all.extend(findings("b", "x.go", 0..30));

    let told = cofiring(&catalog(), &all, &language);

    let go = told
        .iter()
        .find(|c| c.decision == "local/a" && c.by == "local/b" && c.language == "go");
    assert_eq!(go.map(|c| (c.subjects, c.shared)), Some((30, 30)));
    assert!(
        !told
            .iter()
            .any(|c| c.decision == "local/a" && c.by == "local/b" && c.language == "rust"),
        "b has no finding in rust"
    );
}

#[test]
fn likely_implied_by_a_language_where_the_other_covers_it() {
    let mut all = findings("a", "x.go", 0..30);
    all.extend(findings("a", "y.rs", 0..30));
    all.extend(findings("b", "x.go", 0..30));
    all.extend(findings("b", "y.rs", 100..130));

    let told = likely_implied_by(&catalog(), &all, &language, "local/a");

    assert_eq!(told.len(), 1);
    assert_eq!(
        (told[0].language.as_str(), told[0].by.as_str()),
        ("go", "local/b")
    );
}

#[test]
fn likely_implied_by_edges() {
    let edge = |shared: usize, of: usize| {
        let mut all: Vec<Diagnostic> = findings("a", "x.go", 0..of);
        all.extend(findings("b", "x.go", 0..shared));
        likely_implied_by(&catalog(), &all, &language, "local/a")
    };

    assert_eq!(edge(90, 100).len(), 1, "90% of 100 subjects");
    assert!(edge(89, 100).is_empty(), "89% is not likely");
    assert_eq!(edge(20, 20).len(), 1, "20 subjects, all shared");
    assert!(edge(19, 19).is_empty(), "19 subjects are too few");
}

#[test]
fn likely_implied_by_counts_distinct_subjects() {
    let mut all: Vec<Diagnostic> = (0..25).map(|i| finding("a", "x.go", i % 10)).collect();
    all.extend((0..25).map(|i| finding("b", "x.go", i % 10)));

    let told = likely_implied_by(&catalog(), &all, &language, "local/a");

    assert!(
        told.is_empty(),
        "25 findings of 10 subjects are 10 subjects"
    );
    let counted = cofiring(&catalog(), &all, &language);
    assert_eq!(between(&counted, "a", "b").map(|c| c.subjects), Some(10));
}

/// What a run of `dir` found as `(file, subject)`, with the cache `cache`.
fn subjects(dir: &Path, cache: &Path) -> Vec<(String, Option<String>)> {
    let (engine, _) = session(dir).engine().unwrap();
    let run = engine
        .with_cache(cache.to_path_buf(), u64::MAX)
        .check(&[], &[])
        .unwrap();
    let mut found: Vec<_> = run
        .diagnostics
        .iter()
        .filter(|d| d.rule_id == "local/short")
        .map(|d| (d.file.display().to_string(), d.subject.clone()))
        .collect();
    found.sort();
    found
}

#[test]
fn cofiring_of_a_warm_run_is_the_cofiring_of_a_cold_one() {
    let dir = project("warn");
    write(dir.path(), "b.txt", LONG);
    let cache = tempfile::tempdir().unwrap();
    let cache = cache.path().join("cache");

    let cold = subjects(dir.path(), &cache);
    let warm = subjects(dir.path(), &cache);
    write(dir.path(), "b.txt", &format!("{LONG}more\n"));
    let changed = subjects(dir.path(), &cache);

    assert_eq!(cold.len(), 2, "{cold:?}");
    assert!(
        cold.iter().all(|(_, subject)| subject.is_some()),
        "{cold:?}"
    );
    assert_eq!(warm, cold, "the report of the last run");
    assert_eq!(changed, cold, "findings of the graph");
}
