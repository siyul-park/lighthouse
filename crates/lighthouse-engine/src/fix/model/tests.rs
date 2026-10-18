//! The fix by a model: the extent an agent may change, and how its result is
//! judged.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use lighthouse_model::{
    Diagnostic, Fingerprint, Incomplete, Position, Safety, Severity, Span,
    ucm::{File, Fragment, Project, Symbol, SymbolId, SymbolKind, Visibility},
};
use serde_json::json;

use super::{Account, Class, Event, extent, scratch::run_verify, verify::judge};
use crate::{
    Run,
    fix::{
        FixedBy,
        propose::{Aim, Proposal},
    },
};

pub(super) const FILE: &str = "a.go";

fn at(line: u32, col: u32) -> Position {
    Position { line, col }
}

pub(super) fn span(from: (u32, u32), to: (u32, u32)) -> Span {
    Span {
        start: at(from.0, from.1),
        end: at(to.0, to.1),
    }
}

pub(super) fn symbol(name: &str, lines: (u32, u32), end_col: u32) -> Symbol {
    let span = span((lines.0, 1), (lines.1, end_col));
    Symbol {
        id: SymbolId::new("m", &[], name, SymbolKind::Function),
        kind: SymbolKind::Function,
        visibility: Visibility::Public,
        owner: None,
        file: FILE.into(),
        span,
        extent: Some(span),
        doc: None,
        name: name.to_owned(),
        role: None,
        optional: false,
        type_ref: None,
    }
}

fn project(symbols: Vec<Symbol>) -> Project {
    Project::merge([Fragment {
        files: vec![File {
            path: FILE.into(),
            lang: "go".to_owned(),
            hash: String::new(),
            generated: false,
            test: false,
        }],
        symbols,
        ..Fragment::default()
    }])
}

pub(super) fn finding(rule: &str, line: u32, severity: Severity, key: &str) -> Diagnostic {
    let mut finding = Diagnostic::new(
        rule,
        severity,
        "the name is too long",
        FILE,
        span((line, 3), (line, 9)),
        Fingerprint::new(rule, FILE, key),
    );
    finding.evidence = json!({"words": 6});
    finding
}

/// Lines `1..=n` of a file, each `line N`.
fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

#[test]
fn the_extent_is_the_symbol_of_the_finding_else_the_one_around_it_else_a_small_file() {
    let text = "package m\n\nfunc outer() {\n\tfunc inner() {\n\t}\n}\n\nvar x = 1\n";
    let project = project(vec![symbol("outer", (3, 6), 2), symbol("inner", (4, 5), 3)]);
    let choose = |finding: &Diagnostic| extent::choose(&project, finding, text);

    let around = choose(&finding("r", 4, Severity::Warn, "a")).unwrap();
    assert_eq!(around, span((4, 1), (5, 3)));

    let mut named = finding("r", 8, Severity::Warn, "b");
    named.symbol = Some(symbol("outer", (3, 6), 2).id.as_str().to_owned());
    assert_eq!(choose(&named).unwrap(), span((3, 1), (6, 2)));

    let whole = choose(&finding("r", 8, Severity::Warn, "c")).unwrap();
    assert_eq!(
        whole.start,
        at(1, 1),
        "no symbol holds line 8: the small file"
    );
    assert_eq!(whole.end, at(9, 1));
}

#[test]
fn a_large_file_is_rewritten_only_around_a_symbol() {
    let text = numbered(400);
    let error = extent::choose(
        &project(Vec::new()),
        &finding("r", 5, Severity::Warn, "a"),
        &text,
    )
    .unwrap_err();
    assert_eq!(error, "too large for a model fix without a symbol");

    let small = numbered(399);
    assert!(
        extent::choose(
            &project(Vec::new()),
            &finding("r", 5, Severity::Warn, "a"),
            &small
        )
        .is_ok()
    );
}

pub(super) fn proposal(finding: &Diagnostic) -> Proposal {
    Proposal {
        fingerprint: finding.fingerprint.clone(),
        rule_id: finding.rule_id.clone(),
        file: finding.file.clone(),
        line: finding.span.start.line,
        fixer: FixedBy::Model,
        description: "model fix".to_owned(),
        safety: Safety::Suggested,
        edits: Vec::new(),
        files: vec![finding.file.clone()],
        aim: Aim::Finding,
        chain: 0,
        bases: BTreeMap::new(),
    }
}

pub(super) fn run(findings: Vec<Diagnostic>, incomplete: Vec<Incomplete>) -> Run {
    Run {
        diagnostics: findings,
        incomplete,
        ..Run::default()
    }
}

#[test]
fn a_fix_is_kept_when_the_finding_is_gone_and_nothing_loud_is_new() {
    let hit = finding("r", 3, Severity::Warn, "a");
    let before = run(vec![hit.clone()], Vec::new());
    let quiet = finding("other", 4, Severity::Info, "n");
    let after = run(vec![quiet], Vec::new());

    let verdict = judge(&[proposal(&hit)], &[], &before, &after);

    assert!(
        verdict.own.is_empty() && verdict.files.is_empty(),
        "{verdict:?}"
    );
}

#[test]
fn a_fix_that_leaves_the_finding_or_the_rule_in_the_file_did_not_resolve_it() {
    let hit = finding("r", 3, Severity::Warn, "a");
    let before = run(vec![hit.clone()], Vec::new());
    let renamed = finding("r", 3, Severity::Warn, "changed text");
    for after in [
        run(vec![hit.clone()], Vec::new()),
        run(vec![renamed], Vec::new()),
    ] {
        let verdict = judge(&[proposal(&hit)], &[], &before, &after);
        let (class, reason) = &verdict.own[&hit.fingerprint];
        assert_eq!(*class, Class::Unresolved);
        assert_eq!(reason, "did not resolve the finding");
        assert_eq!(verdict.files[&PathBuf::from(FILE)], *reason);
    }
}

#[test]
fn a_fix_that_adds_a_warning_or_an_error_to_the_file_introduced_it() {
    let hit = finding("r", 3, Severity::Warn, "a");
    let before = run(vec![hit.clone()], Vec::new());
    let new = finding("design/other", 7, Severity::Error, "z");
    let verdict = judge(&[proposal(&hit)], &[], &before, &run(vec![new], Vec::new()));
    let (class, reason) = &verdict.own[&hit.fingerprint];
    assert_eq!(*class, Class::Introduced);
    assert_eq!(reason, "introduced design/other at line 7");
}

#[test]
fn a_fix_that_leaves_its_file_partly_analyzed_is_not_kept() {
    let hit = finding("r", 3, Severity::Warn, "a");
    let before = run(vec![hit.clone()], Vec::new());
    let gap = Incomplete {
        path: Some(FILE.into()),
        reason: "syntax error".to_owned(),
    };
    let verdict = judge(&[proposal(&hit)], &[], &before, &run(Vec::new(), vec![gap]));
    let (class, reason) = &verdict.own[&hit.fingerprint];
    assert_eq!(*class, Class::Incomplete);
    assert_eq!(reason, "analysis incomplete: syntax error");
}

#[test]
fn only_fixes_by_a_model_are_judged() {
    let hit = finding("r", 3, Severity::Warn, "a");
    let before = run(vec![hit.clone()], Vec::new());
    let mut rule_made = proposal(&hit);
    rule_made.fixer = FixedBy::Fixer("design/short-names".to_owned());
    let verdict = judge(&[rule_made], &[], &before, &run(vec![hit], Vec::new()));
    assert!(verdict.own.is_empty());
}

#[test]
fn counts_are_kept_per_decision_and_declines_by_class() {
    let account = Account::default();
    for event in [
        Event::Asked,
        Event::Asked,
        Event::Answered,
        Event::Verified,
        Event::Applied,
        Event::Declined(Class::Unresolved),
        Event::Declined(Class::Unresolved),
        Event::Declined(Class::Unchanged),
    ] {
        account.add("a", event);
    }
    account.add("b", Event::Previewed);

    let counts = account.counts();

    assert_eq!(
        counts.keys().collect::<Vec<_>>(),
        ["a", "b"].iter().collect::<Vec<_>>()
    );
    let a = &counts["a"];
    assert_eq!((a.asked, a.answered, a.verified, a.applied), (2, 1, 1, 1));
    assert_eq!(a.declined["unresolved"], 2);
    assert_eq!(a.declined["unchanged"], 1);
    assert_eq!(counts["b"].previewed, 1);
}

#[test]
fn the_classes_of_declines_are_named() {
    let names: BTreeSet<_> = [
        Class::NoExtent,
        Class::Unanswered,
        Class::OutOfScope,
        Class::Conflict,
        Class::Unchanged,
        Class::Unresolved,
        Class::Introduced,
        Class::Incomplete,
        Class::RolledBack,
    ]
    .map(Class::name)
    .into();
    assert_eq!(names.len(), 9);
}

#[test]
fn the_verify_commands_run_over_a_copy_that_holds_the_fixed_texts() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), "old\n").unwrap();
    fs::write(root.path().join(".gitignore"), "skipped/\n").unwrap();
    fs::create_dir(root.path().join("skipped")).unwrap();
    fs::write(root.path().join("skipped/b.txt"), "x\n").unwrap();
    let texts = crate::Overlays::from([(PathBuf::from("a.txt"), "new\n".to_owned())]);
    let sh = |script: &str| vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()];
    let limit = std::time::Duration::from_secs(20);

    let passed = run_verify(
        root.path(),
        &texts,
        &[sh("grep -q new a.txt"), sh("test ! -e skipped/b.txt")],
        limit,
    );
    assert_eq!(passed, Ok(()));
    assert_eq!(
        fs::read_to_string(root.path().join("a.txt")).unwrap(),
        "old\n"
    );

    let failed = run_verify(
        root.path(),
        &texts,
        &[sh("echo one >&2; echo two >&2; exit 3"), sh("exit 0")],
        limit,
    )
    .unwrap_err();
    assert!(failed.contains("exited 3"), "{failed}");
    assert!(failed.ends_with("one\ntwo"), "{failed}");

    let escaping = crate::Overlays::from([(PathBuf::from("../x"), String::new())]);
    assert!(run_verify(root.path(), &escaping, &[], limit).is_err());
}
