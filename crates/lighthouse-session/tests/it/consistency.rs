//! A decision checked against the decisions in force: nothing is written, the
//! decisions that report on the same subjects are told as likely implying it,
//! and what proof there is of an order contradiction is a CLI matter.

use lighthouse_report::Detail;
use lighthouse_session::consistency::{
    self, Basis, Contradiction, DecisionCheck, Kind, Refused, Scope, Target, Verdict,
};
use serde_json::{Value, json};

use crate::fixture::{LONG, project, session, trusted, write};

/// How many files the project has beyond the one its decision fails.
const NOTES: usize = 25;

/// A decision that fails the same files as `local/short` under another name.
fn again() -> Value {
    json!({
        "kind": "Decision",
        "metadata": { "name": "local/also-short" },
        "spec": {
            "title": "Notes stay short",
            "context": "Notes are read at a glance.",
            "scope": { "subject": "file" },
            "requirement": "A note MUST NOT run past two lines.",
            "severity": "warn",
            "check": {
                "type": "cel",
                "select": "file",
                "where": "file.path.endsWith(\".txt\") && file.lines > 2",
                "message": "{{ file.path }} is too long"
            },
            "examples": [
                { "name": "long", "language": "text", "kind": "invalid",
                  "files": [{ "path": "a.txt", "body": "a\nb\nc\n" }], "expect": [{ "line": 1 }] },
                { "name": "short", "language": "text", "kind": "valid",
                  "files": [{ "path": "a.txt", "body": "a\nb\n" }] }
            ]
        }
    })
}

#[test]
fn a_decision_that_reports_on_the_same_subjects_is_likely_implied() {
    let dir = project("warn");
    for n in 0..NOTES {
        write(dir.path(), &format!("note-{n}.txt"), LONG);
    }

    let checked = consistency::check_decision(
        &session(dir.path()),
        Target::Document(again()),
        Detail::Compact,
    )
    .unwrap();

    assert_eq!(checked.verdict, Verdict::Independent);
    assert_eq!(checked.basis, Basis::Empirical);
    assert!(checked.implied.is_empty(), "no proof of implication");
    let told: Vec<_> = checked
        .likely_implied_by
        .iter()
        .map(|l| (l.by.as_str(), l.language.as_str(), l.cofire, l.subjects))
        .collect();
    assert_eq!(told, [("local/short", "text", 100.0, NOTES + 1)]);
    assert!(
        !dir.path()
            .join(".lighthouse/decisions/also-short.yaml")
            .exists(),
        "a check never writes the document"
    );
}

#[test]
fn a_decision_with_few_subjects_is_not_likely_implied() {
    let dir = project("warn");

    let checked = consistency::check_decision(
        &session(dir.path()),
        Target::Document(again()),
        Detail::Compact,
    )
    .unwrap();

    assert!(checked.likely_implied_by.is_empty());
}

#[test]
fn a_decision_of_the_catalog_is_checked_as_it_is() {
    let dir = project("warn");

    let checked = consistency::check_decision(
        &session(dir.path()),
        Target::Id("local/short".to_owned()),
        Detail::Compact,
    )
    .unwrap();

    assert_eq!(checked.decision, "local/short");
    assert_eq!(checked.verdict, Verdict::Independent);
    assert_eq!(checked.tier, "F");
}

#[test]
fn an_unknown_decision_or_a_document_that_is_not_one_is_an_error() {
    let dir = project("warn");
    let session = session(dir.path());

    assert!(
        consistency::check_decision(
            &session,
            Target::Id("nope/nope".to_owned()),
            Detail::Compact
        )
        .is_err()
    );
    assert!(
        consistency::check_decision(
            &session,
            Target::Document(json!({ "kind": "Project" })),
            Detail::Compact
        )
        .is_err()
    );
    assert!(
        consistency::check_decision(
            &session,
            Target::Document(json!({ "kind": "Decision", "metadata": { "name": "design/x" } })),
            Detail::Compact
        )
        .is_err(),
        "only a local decision is a document"
    );
}

fn contradiction(with: &[&str], resolved_by: Option<&str>) -> Contradiction {
    Contradiction {
        language: "go".to_owned(),
        with: with.iter().map(|w| (*w).to_owned()).collect(),
        kind: Kind::Order,
        basis: Basis::Proven,
        witness: json!({ "shape": "a public type A, a public function B" }),
        resolved_by: resolved_by.map(str::to_owned),
    }
}

#[test]
fn refused() {
    let mut check = blank();
    check.decision = "local/new".to_owned();
    check.contradicts = vec![contradiction(&["local/a"], None)];
    let found = check.introduced_since(None);

    let refused = Refused::new(&check, &found, None);

    assert_eq!(refused.found.len(), 1);
    assert_eq!(
        refused.to_string(),
        format!(
            "`local/new` was not written, it contradicts the decisions in force:\n{}",
            contradiction(&["local/a"], None).told()
        )
    );
}

#[test]
fn refused_old_incomplete() {
    let mut check = blank();
    check.decision = "local/new".to_owned();
    check.contradicts = vec![contradiction(&["local/a"], None)];
    let mut was = blank();
    was.incomplete = vec!["go: the search was cut off".to_owned()];
    let found = check.introduced_since(Some(&was));

    let refused = Refused::new(&check, &found, Some(&was));

    assert!(
        refused
            .to_string()
            .contains("the old version could not be searched in full (go: the search was cut off)"),
        "{refused}"
    );
    assert!(
        refused
            .check
            .incomplete
            .contains(&"the old version could not be searched in full".to_owned())
    );
}

#[test]
fn contradiction_told() {
    let told = contradiction(&["local/a", "local/b"], None).told();

    assert_eq!(
        told,
        "contradicts [go] with local/a, local/b: on code shaped like a public type A, a public function B, no order satisfies them"
    );
    assert!(
        contradiction(&["local/a"], Some("local/a"))
            .told()
            .ends_with("; settled, local/a supersedes another")
    );
}

#[test]
fn decision_check_contradicted() {
    let mut check = blank();
    assert!(!check.contradicted());

    check.contradicts = vec![contradiction(&["local/a"], Some("local/a"))];
    assert!(!check.contradicted(), "a decision settles it");

    check.contradicts = vec![contradiction(&["local/a"], None)];
    assert!(check.contradicted());
}

#[test]
fn decision_check_introduced_since() {
    let mut now = blank();
    now.contradicts = vec![
        contradiction(&["local/a"], None),
        contradiction(&["local/b"], None),
        contradiction(&["local/c"], Some("local/c")),
    ];
    let mut was = blank();
    was.contradicts = vec![contradiction(&["local/a"], None)];

    let new = now.introduced_since(Some(&was));

    assert_eq!(new.len(), 1, "{new:?}");
    assert_eq!(new[0].with, ["local/b"]);
    assert_eq!(
        now.introduced_since(None).len(),
        2,
        "a created one has no past"
    );
}

#[test]
fn decision_check_introduced_since_reopened() {
    let mut now = blank();
    now.contradicts = vec![contradiction(&["local/a"], None)];
    let mut was = blank();
    was.contradicts = vec![contradiction(&["local/a"], Some("local/new"))];

    let new = now.introduced_since(Some(&was));

    assert_eq!(
        new.len(),
        1,
        "a settled one that is reopened is new: {new:?}"
    );
}

#[test]
fn decision_check_notice() {
    let mut check = blank();
    assert_eq!(check.notice(), None);

    check.interferes = vec![interference()];
    check.incomplete = vec!["go: no renderer".to_owned()];
    let told = check.notice().unwrap();

    assert!(told.contains("interferes with local/short (ok)"), "{told}");
    assert!(told.contains("not searched: go: no renderer"), "{told}");
}

#[test]
fn decision_check_worst_verdict() {
    let mut check = blank();
    assert_eq!(check.worst_verdict(), Verdict::Independent);

    check.interferes = vec![interference()];
    assert_eq!(check.worst_verdict(), Verdict::Interferes);

    check.incomplete = vec!["go: no renderer".to_owned()];
    assert_eq!(check.worst_verdict(), Verdict::Incomplete);
}

#[test]
fn decision_check_weakest_basis() {
    let mut check = blank();
    assert_eq!(
        check.weakest_basis(),
        Basis::Empirical,
        "nothing but co-firing ran"
    );

    check.bounded = vec![scope_of("go")];
    assert_eq!(check.weakest_basis(), Basis::Bounded, "a search finished");

    check.incomplete = vec!["go: no renderer".to_owned()];
    assert_eq!(check.weakest_basis(), Basis::Incomplete);
}

#[test]
fn check_decision() {
    let dir = project("warn");

    let checked = consistency::check_decision(
        &session(dir.path()),
        Target::Id("local/short".to_owned()),
        Detail::Full,
    )
    .unwrap();

    assert_eq!(checked.decision, "local/short");
}

#[test]
fn decision_check() {
    let dir = project("warn");

    let checked = consistency::check_decision(
        &session(dir.path()),
        Target::Document(again()),
        Detail::Compact,
    )
    .unwrap();

    let told = serde_json::to_value(&checked).unwrap();
    assert_eq!(told["decision"], "local/also-short");
    assert_eq!(told["implied"], json!([]), "kept so the shape is stable");
    assert!(told.get("bounded").is_none(), "told only when searched");
}

#[test]
fn scope() {
    let dir = project("warn");

    let checked = consistency::check_decision(
        &session(dir.path()),
        Target::Id("design/top-down-order".to_owned()),
        Detail::Compact,
    )
    .unwrap();

    for scope in &checked.bounded {
        assert!(scope.declarations >= 2, "{scope:?}");
        assert!(!scope.language.is_empty());
    }
}

#[test]
fn decision_check_code() {
    let dir = project("warn");

    let checked = consistency::check_decision(
        &session(dir.path()),
        Target::Document(again()),
        Detail::Compact,
    )
    .unwrap();

    assert_eq!(checked.code(), 0);
}

#[test]
fn overlay() {
    let dir = project("warn");

    let (candidate, id) =
        consistency::overlay(&session(dir.path()), Target::Document(again())).unwrap();

    assert_eq!(id, "local/also-short");
    assert!(candidate.catalog.decision(&id).is_some());
    assert!(
        !dir.path()
            .join(".lighthouse/decisions/also-short.yaml")
            .exists(),
        "the layer is in memory"
    );
}

#[test]
fn gaps() {
    let dir = project("warn");
    let (candidate, id) =
        consistency::overlay(&session(dir.path()), Target::Document(again())).unwrap();
    let decision = candidate.catalog.decision(&id).unwrap();

    let told = consistency::gaps(&candidate, decision).unwrap();

    assert!(
        told.is_empty(),
        "no enabled decision is beyond formal reach"
    );
}

#[test]
fn held_to() {
    let dir = project("warn");
    let (candidate, id) =
        consistency::overlay(&session(dir.path()), Target::Document(again())).unwrap();
    let decision = candidate.catalog.decision(&id).unwrap();

    let held = consistency::held_to(&candidate, decision, &["go"]);

    assert!(
        held.contains(&"design/top-down-order".to_owned()),
        "{held:?}"
    );
    assert!(!held.contains(&id), "never to itself");
    assert!(consistency::held_to(&candidate, decision, &[]).is_empty());
}

#[test]
fn likely_implied() {
    let dir = project("warn");
    for n in 0..NOTES {
        write(dir.path(), &format!("note-{n}.txt"), LONG);
    }
    let (candidate, id) =
        consistency::overlay(&session(dir.path()), Target::Document(again())).unwrap();

    let (told, unfinished) = consistency::likely_implied(&candidate, &id).unwrap();

    assert_eq!(told.len(), 1);
    assert_eq!(told[0].by, "local/short");
    assert_eq!(unfinished, None, "the run of the project finished");
}

/// A check that found nothing.
fn blank() -> DecisionCheck {
    DecisionCheck {
        decision: "local/x".to_owned(),
        tier: "F".to_owned(),
        tier_reason: "translated".to_owned(),
        ..DecisionCheck::default()
    }
}

fn scope_of(language: &str) -> Scope {
    Scope {
        language: language.to_owned(),
        declarations: 2,
        alphabet: "a".to_owned(),
        renderer: "r".to_owned(),
        options: Vec::new(),
    }
}

fn interference() -> consistency::Interference {
    consistency::Interference {
        language: "text".to_owned(),
        with: "local/short".to_owned(),
        example: "ok".to_owned(),
        introduced: false,
        places: vec!["a.txt:1".to_owned()],
        message: "too long".to_owned(),
    }
}

#[test]
fn a_conflict_an_example_declares_is_told_as_a_record() {
    let dir = project("warn");
    let mut document = again();
    document["spec"]["check"]["where"] = json!("file.path.endsWith(\".txt\") && file.lines > 3");
    document["spec"]["examples"] = json!([
        { "name": "long", "language": "text", "kind": "invalid",
          "files": [{ "path": "a.txt", "body": "a\nb\nc\nd\n" }], "expect": [{ "line": 1 }] },
        { "name": "ok", "language": "text", "kind": "valid",
          "files": [{ "path": "a.txt", "body": "a\nb\nc\n" }],
          "conflicts": ["local/short"] }
    ]);

    let checked = consistency::check_decision(
        &session(dir.path()),
        Target::Document(document),
        Detail::Compact,
    )
    .unwrap();

    let told = serde_json::to_value(&checked.interferes).unwrap();
    assert_eq!(told[0]["with"], "local/short", "{told}");
    assert_eq!(told[0]["example"], "ok");
    assert_eq!(told[0]["introduced"], false);
    assert_eq!(checked.verdict, Verdict::Interferes);
    assert_eq!(checked.code(), 0);
}

#[test]
fn a_decision_that_names_a_program_is_not_trusted_unseen() {
    let dir = project("warn");
    let session = trusted(dir.path());
    let mut document = again();
    document["spec"]["examples"][0]["fixed"] = json!([{ "path": "a.txt", "body": "a\nb\n" }]);
    document["spec"]["fix"] = json!({
        "type": "command", "safety": "unsafe", "output": "stdout",
        "command": ["sh", "-c", "cat \"$1\"", "sh", "{file}"]
    });

    let (plain, _) = consistency::overlay(&session, Target::Document(again())).unwrap();
    let (running, _) = consistency::overlay(&session, Target::Document(document)).unwrap();

    assert!(session.trusted());
    assert!(plain.trusted(), "it adds no program to what the user saw");
    assert!(
        !running.trusted(),
        "a program of its own is not trusted unseen"
    );
}
