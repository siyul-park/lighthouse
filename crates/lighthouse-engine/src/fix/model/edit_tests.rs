//! The fix by an agent that edits: its brief, the scope, the verdict on the edit of a
//! file, and the scratch copy it works in.

use std::path::Path;

use lighthouse_model::{Diagnostic, Fingerprint, Severity};
use lighthouse_plugin::FixDecision;
use lighthouse_spec::{Example, ExampleFile, ExampleKind, FixScope};

use super::{
    Class,
    tests::{FILE, finding, proposal, run},
    verify::{Verdict, judge},
};
use crate::{
    Run,
    fix::{
        brief::{Target, brief},
        propose::{Aim, Proposal},
    },
};

fn decision(id: &str) -> FixDecision {
    FixDecision {
        id: id.to_owned(),
        requirement: format!("{id} requirement"),
        context: String::new(),
    }
}

fn example() -> Example {
    Example {
        name: "short".to_owned(),
        language: "go".to_owned(),
        kind: ExampleKind::Invalid,
        files: vec![ExampleFile::inline("x.go", "bad\n")],
        canonical: false,
        expect: Vec::new(),
        options: serde_json::Map::new(),
        fixed: vec![ExampleFile::inline("x.go", "good\n")],
    }
}

fn target<'a>(
    finding: &'a Diagnostic,
    decision: &'a FixDecision,
    instruction: Option<&'a str>,
    examples: Vec<Example>,
    scope: FixScope,
) -> Target<'a> {
    Target {
        decision,
        instruction,
        examples,
        scope,
        message: &finding.message,
        evidence: &finding.evidence,
        place: format!(
            "{}:{}:{}",
            finding.file.display(),
            finding.span.start.line,
            finding.span.start.col
        ),
    }
}

#[test]
fn the_brief_lists_the_findings_of_the_file_with_their_decisions_and_asks_for_a_line_each() {
    let (names, flat) = (decision("design/short-names"), decision("design/flat"));
    let (first, second, third) = (
        finding("design/short-names", 3, Severity::Warn, "a"),
        finding("design/flat", 7, Severity::Warn, "b"),
        finding("design/short-names", 9, Severity::Warn, "c"),
    );
    let targets = [
        target(
            &first,
            &names,
            Some("Rename, do not delete."),
            vec![example()],
            FixScope::Symbol,
        ),
        target(&second, &flat, None, Vec::new(), FixScope::Files),
        target(
            &third,
            &names,
            Some("Rename, do not delete."),
            vec![example()],
            FixScope::Symbol,
        ),
    ];

    let text = brief(&targets, &[]);

    let want = "You are fixing findings of design rules in the files of this directory. Change only what the findings need, keep behaviour and style, stay within the scope of each finding, and do not run commands that change anything outside this directory.
Scope `symbol`: change only the symbol the finding is in (the whole file when it is short and no symbol holds it) and touch no other file. Scope `files`: change any file of this directory the fix needs, such as the callers of a signature you change.

Finding 1
Decision: design/short-names
Requirement: design/short-names requirement
Instruction: Rename, do not delete.
Message: the name is too long
Evidence: {\"words\":6}
Where: a.go:3:3
Scope: symbol

Example 1 (x.go), before:
```go
bad
```

Example 1 (x.go), after:
```go
good
```

Finding 2
Decision: design/flat
Requirement: design/flat requirement
Message: the name is too long
Evidence: {\"words\":6}
Where: a.go:7:3
Scope: files

Finding 3
Decision: design/short-names
Requirement: design/short-names requirement
Instruction: Rename, do not delete.
Message: the name is too long
Evidence: {\"words\":6}
Where: a.go:9:3
Scope: symbol

Lighthouse checks the result by analyzing the files again: each finding must be gone, and nothing new of warning or error severity may appear in the files you change or in the files that use a symbol you change.
When done, reply with one line per finding: its number and `fixed` or `left` with a reason.
";
    assert_eq!(text, want);
    assert_eq!(text, brief(&targets, &[]));
}

fn shared(finding: &Diagnostic) -> Proposal {
    Proposal {
        aim: Aim::File,
        ..proposal(finding)
    }
}

/// The verdict on the edit of one file for `findings`, the first of which is
/// kept and the others share its edits, after the check `after`.
fn judged(findings: &[Diagnostic], after: &Run) -> Verdict {
    let kept = [shared(&findings[0])];
    let merged: Vec<(Proposal, Fingerprint)> = findings[1..]
        .iter()
        .map(|f| (shared(f), findings[0].fingerprint.clone()))
        .collect();
    let before = run(findings.to_vec(), Vec::new());
    judge(&kept, &merged, &before, after)
}

#[test]
fn an_edit_is_kept_when_one_of_its_findings_is_gone_and_the_others_are_declined_one_by_one() {
    let (a, b) = (
        finding("r", 3, Severity::Warn, "a"),
        finding("r", 5, Severity::Warn, "b"),
    );

    let verdict = judged(&[a.clone(), b.clone()], &run(vec![b.clone()], Vec::new()));
    assert!(verdict.files.is_empty(), "{verdict:?}");
    assert_eq!(verdict.own.len(), 1);
    assert_eq!(verdict.own[&b.fingerprint].0, Class::Unresolved);

    let verdict = judged(&[a.clone(), b.clone()], &run(vec![a.clone()], Vec::new()));
    assert!(verdict.files.is_empty(), "{verdict:?}");
    assert_eq!(verdict.own[&a.fingerprint].0, Class::Unresolved);

    let verdict = judged(&[a, b], &run(Vec::new(), Vec::new()));
    assert!(verdict.own.is_empty() && verdict.files.is_empty());
}

#[test]
fn an_edit_that_fixes_none_of_its_findings_or_adds_a_warning_is_rolled_back_whole() {
    let (a, b) = (
        finding("r", 3, Severity::Warn, "a"),
        finding("r", 5, Severity::Warn, "b"),
    );

    let still = run(vec![a.clone(), b.clone()], Vec::new());
    let verdict = judged(&[a.clone(), b.clone()], &still);
    assert_eq!(verdict.files.len(), 1);
    assert_eq!(verdict.own.len(), 2);

    let new = finding("design/other", 9, Severity::Warn, "z");
    let verdict = judged(&[a, b], &run(vec![new], Vec::new()));
    assert_eq!(
        verdict.files[Path::new(FILE)],
        "introduced design/other at line 9"
    );
    assert_eq!(verdict.own.len(), 2);
    assert!(
        verdict
            .own
            .values()
            .all(|(class, _)| *class == Class::Introduced)
    );
}

#[test]
fn a_finding_that_only_changed_its_fingerprint_with_a_rename_is_not_new() {
    let target = finding("r", 3, Severity::Warn, "a");
    let side = finding("design/other", 9, Severity::Warn, "old name");
    let renamed = finding("design/other", 9, Severity::Warn, "new name");
    let before = run(vec![target.clone(), side.clone()], Vec::new());
    let verdict =
        |after: Vec<Diagnostic>| judge(&[shared(&target)], &[], &before, &run(after, Vec::new()));

    let kept = verdict(vec![renamed.clone()]);
    assert!(kept.files.is_empty() && kept.own.is_empty(), "{kept:?}");

    let twice = verdict(vec![
        renamed.clone(),
        finding("design/other", 12, Severity::Warn, "x"),
    ]);
    assert!(
        twice.files[Path::new(FILE)].starts_with("introduced design/other"),
        "{twice:?}"
    );
}

#[test]
fn a_new_finding_of_the_rule_stands_for_one_of_the_old_whose_fingerprint_changed() {
    let (a, b) = (
        finding("r", 3, Severity::Info, "a"),
        finding("r", 5, Severity::Info, "b"),
    );
    let moved = finding("r", 5, Severity::Info, "moved");

    let verdict = judged(&[a, b], &run(vec![moved], Vec::new()));

    assert!(verdict.files.is_empty(), "{verdict:?}");
    assert_eq!(verdict.own.len(), 1, "one of the two is explained by it");
}
