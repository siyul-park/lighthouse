//! Asking what an edit would do: the verdict follows what fires, and nothing
//! is remembered.

use lighthouse_report::Detail;
use lighthouse_session::{
    CheckRequest, Checked,
    ask::{self, Answer, Delta, Verdict, WhatIf},
    check,
};

use crate::fixture::{project, trusted, write};

/// The diff that makes `a.txt` a long note.
const LENGTHEN: &str = "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,3 @@\n a\n b\n+c\n";

/// The diff that changes a line of `a.txt` and leaves it short.
const REWORD: &str = "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n-a\n+z\n b\n";

/// A project with a short note, and the answer to `edit` over it.
fn asked(severity: &str, edit: &str) -> Answer {
    let dir = project(severity);
    write(dir.path(), "a.txt", "a\nb\n");
    let session = trusted(dir.path());
    let question = WhatIf {
        edit,
        paths: &[],
        detail: Detail::Full,
    };
    let mut run = |request: CheckRequest| -> lighthouse_session::Result<Checked> {
        check(&session, &request)
    };
    ask::what_if(&session, &question, &mut run).unwrap()
}

#[test]
fn what_if() {
    let answer = asked("error", LENGTHEN);

    let Delta::Full(fired) = &answer.fired else {
        panic!("a full answer lists the findings");
    };
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].decision, "local/short");
    assert_eq!(answer.unchanged, 0);
}

#[test]
fn answer() {
    let answer = asked("error", REWORD);

    assert_eq!(answer.verdict, Verdict::Permitted);
    assert!(matches!(&answer.fired, Delta::Full(items) if items.is_empty()));
    assert!(matches!(&answer.cleared, Delta::Full(items) if items.is_empty()));
}

#[test]
fn verdict() {
    assert_eq!(asked("error", LENGTHEN).verdict, Verdict::Forbidden);
    assert_eq!(asked("warn", LENGTHEN).verdict, Verdict::Forbidden);
    assert_eq!(asked("info", LENGTHEN).verdict, Verdict::Permitted);
    assert_eq!(Verdict::Forbidden.to_string(), "forbidden");
}

#[test]
fn code() {
    assert_eq!(asked("error", LENGTHEN).code(), 1);
    assert_eq!(asked("error", REWORD).code(), 0);
}

#[test]
fn premise() {
    let answer = asked("error", LENGTHEN);

    assert_eq!(answer.premises.len(), 1);
    assert_eq!(answer.premises[0].decision, "local/short");
    assert_eq!(answer.premises[0].severity.to_string(), "error");
}
