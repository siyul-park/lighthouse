//! Judgments and suppressions: standings, expiry and what a finding asks of review.

use std::{fs, thread};

use crate::support::*;
use lighthouse_model::{Judgment, Severity};
use lighthouse_store::{Error, Standing, State, StatusFilter, Store};
use rusqlite::Connection;
use serde_json::json;

#[test]
fn judgments_and_suppressions_are_append_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    judge_suppressed(&mut store, "f1", "named policy");

    let raw = Connection::open(Store::path_in(dir.path())).unwrap();
    for table in ["judgments", "suppressions"] {
        let update = raw.execute(&format!("UPDATE {table} SET fingerprint = 'x'"), []);
        assert!(update.unwrap_err().to_string().contains("append-only"));
        let delete = raw.execute(&format!("DELETE FROM {table}"), []);
        assert!(delete.unwrap_err().to_string().contains("append-only"));
    }
    assert_eq!(store.history("f1").unwrap().len(), 1);
}

#[test]
fn standings_follow_the_latest_judgment() {
    let mut store = memory_with(&[
        observed("pass", "design/a", "a.go"),
        observed("fail", "design/a", "b.go"),
        observed("left", "design/a", "c.go"),
        observed("broad", "design/a", "d.go"),
        observed("reconsidered", "design/a", "e.go"),
        observed("unjudged", "design/a", "f.go"),
    ]);
    judge(&mut store, "pass", Judgment::Pass);
    judge(&mut store, "fail", Judgment::Fail);
    judge_suppressed(&mut store, "left", "project-allowed");
    judge(&mut store, "broad", Judgment::NotApplicable);
    judge_suppressed(&mut store, "reconsidered", "named policy");
    let subjects: Vec<_> = ["pass", "fail", "left", "broad", "reconsidered", "unjudged"]
        .iter()
        .map(|f| observed(f, "design/a", "x.go").subject())
        .collect();
    let standings = store.standings_for(&subjects).unwrap();
    let keys: Vec<_> = standings.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["broad", "fail", "left", "pass", "reconsidered"],
        "a finding nobody judged has no standing"
    );
    assert_eq!(standings["pass"].standing, Standing::Suppressed);
    assert_eq!(standings["broad"].standing, Standing::Suppressed);
    assert_eq!(standings["left"].standing, Standing::Suppressed);
    assert_eq!(
        standings["left"].justification.as_deref(),
        Some("project-allowed")
    );
    assert_eq!(standings["fail"].standing, Standing::Judged);
    assert_eq!(standings["fail"].judgment, Judgment::Fail);
    assert!(standings["fail"].justification.is_none());

    judge(&mut store, "reconsidered", Judgment::Fail);
    assert_eq!(standing_of(&store, "reconsidered"), Some(Standing::Judged));
    assert_eq!(store.finding("reconsidered").unwrap().state, State::Open);
    assert_eq!(store.finding("pass").unwrap().state, State::Suppressed);
    assert_eq!(store.finding("fail").unwrap().state, State::Open);
}

#[test]
fn a_judgment_expires_when_the_decision_or_the_evidence_changes() {
    let mut store = memory_with(&[
        observed("rule", "design/a", "a.go"),
        observed("evidence", "design/a", "b.go"),
        observed("same", "design/a", "c.go"),
    ]);
    for fingerprint in ["rule", "evidence", "same"] {
        judge_suppressed(&mut store, fingerprint, "intentional-exception");
    }
    assert_eq!(standing_of(&store, "same"), Some(Standing::Suppressed));

    let mut rule = observed("rule", "design/a", "a.go");
    rule.meaning_version = Some("sem2".to_owned());
    let mut evidence = observed("evidence", "design/a", "b.go");
    evidence.evidence = json!({ "fan_out": 20 });
    let same = observed("same", "design/a", "c.go");
    store.record(&run(vec![rule, evidence, same])).unwrap();

    assert_eq!(standing_of(&store, "rule"), Some(Standing::RuleChanged));
    assert_eq!(
        standing_of(&store, "evidence"),
        Some(Standing::EvidenceChanged)
    );
    assert_eq!(standing_of(&store, "same"), Some(Standing::Suppressed));
    assert_eq!(
        fingerprints(&store, &status(StatusFilter::Open)),
        ["rule", "evidence"].map(String::from)
    );
    assert_eq!(
        fingerprints(&store, &status(StatusFilter::Suppressed)),
        ["same"]
    );
}

#[test]
fn evidence_that_only_reflows_does_not_expire_a_judgment() {
    let mut first = observed("f1", "design/a", "a.go");
    first.evidence = json!({ "callee": "a  b\n c", "n": 1 });
    let mut store = memory_with(&[first]);
    judge(&mut store, "f1", Judgment::Pass);
    let mut reflowed = observed("f1", "design/a", "a.go");
    reflowed.evidence = json!({ "n": 1, "callee": "a b c" });
    store.record(&run(vec![reflowed])).unwrap();
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));
}

#[test]
fn a_judgment_hides_by_the_authored_severity_of_the_finding_not_by_its_reported_one() {
    let mut error = observed("f1", "design/a", "a.go");
    error.severity = Severity::Error;
    error.authored_severity = Severity::Warn;
    let mut store = memory_with(&[error]);
    judge(&mut store, "f1", Judgment::Pass);
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));
    assert!(
        !store.finding("f1").unwrap().needs_review(),
        "judged findings do not ask for review"
    );
}

#[test]
fn errors_are_never_hidden_by_a_judgment() {
    let mut error = observed("f1", "design/a", "a.go");
    error.severity = Severity::Warn;
    error.authored_severity = Severity::Error;
    let mut store = memory_with(&[error]);
    judge_suppressed(&mut store, "f1", "intentional-exception");
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Unsuppressible));
    assert_eq!(
        store.history("f1").unwrap().len(),
        1,
        "the judgment is still recorded"
    );
    assert_eq!(fingerprints(&store, &status(StatusFilter::Open)), ["f1"]);
    assert!(fingerprints(&store, &status(StatusFilter::Suppressed)).is_empty());
}

#[test]
fn narrowing_lists_the_findings_judged_not_applicable() {
    let mut store = memory_with(&[
        observed("broad", "design/a", "a.go"),
        observed("wrong", "design/a", "b.go"),
    ]);
    judge(&mut store, "broad", Judgment::NotApplicable);
    judge(&mut store, "wrong", Judgment::Pass);
    assert_eq!(
        fingerprints(&store, &status(StatusFilter::Narrowing)),
        ["broad"]
    );
    assert_eq!(
        fingerprints(&store, &status(StatusFilter::Suppressed)),
        ["broad", "wrong"]
    );
    assert!(store.finding("broad").unwrap().narrowing);
    assert!(!store.finding("wrong").unwrap().narrowing);
}

#[test]
fn as_str_names_each_standing() {
    let names = [
        (Standing::Suppressed, "suppressed"),
        (Standing::Judged, "judged"),
        (Standing::RuleChanged, "rule-changed"),
        (Standing::EvidenceChanged, "evidence-changed"),
        (Standing::Unsuppressible, "unsuppressible"),
    ];
    for (standing, name) in names {
        assert_eq!(standing.as_str(), name);
    }
}

#[test]
fn a_finding_asks_for_review_when_its_decision_did_not_author_an_error_and_nobody_judged_it() {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    let finding = |store: &Store| store.finding("f1").unwrap();
    assert!(finding(&store).needs_review(), "authored info");

    let mut definitive = observed("f1", "design/a", "a.go");
    definitive.authored_severity = Severity::Error;
    store.record(&run(vec![definitive])).unwrap();
    assert!(!finding(&store).needs_review());

    let mut review = observed("f1", "design/a", "a.go");
    review.authored_severity = Severity::Warn;
    review.severity = Severity::Error;
    store.record(&run(vec![review])).unwrap();
    assert!(
        finding(&store).needs_review(),
        "whatever the reported level"
    );

    judge(&mut store, "f1", Judgment::Fail);
    assert!(!finding(&store).needs_review(), "judged");

    let mut moved = observed("f1", "design/a", "a.go");
    moved.meaning_version = Some("sem2".to_owned());
    store.record(&run(vec![moved])).unwrap();
    assert!(finding(&store).needs_review(), "the judgment expired");
}

#[test]
fn changing_only_how_a_decision_is_checked_keeps_its_judgments_and_changing_what_it_means_expires_them()
 {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    judge(&mut store, "f1", Judgment::Pass);

    // Same meaning, another check: the judgment stands.
    let mut rechecked = observed("f1", "design/a", "a.go");
    rechecked.check_revision = Some("chk2".to_owned());
    store.record(&run(vec![rechecked])).unwrap();
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));

    // Another meaning: the judgment is asked again.
    let mut reworded = observed("f1", "design/a", "a.go");
    reworded.meaning_version = Some("sem2".to_owned());
    store.record(&run(vec![reworded])).unwrap();
    assert_eq!(standing_of(&store, "f1"), Some(Standing::RuleChanged));
}

#[test]
fn the_model_that_decided_a_finding_is_recorded_with_the_finding_and_the_judgment_and_never_compared()
 {
    let mut decided = observed("f1", "design/a", "a.go");
    decided.model_version = Some("fake/any@111".to_owned());
    let mut store = memory_with(&[decided]);
    judge(&mut store, "f1", Judgment::Pass);
    assert_eq!(
        store.finding("f1").unwrap().model_version.as_deref(),
        Some("fake/any@111")
    );
    assert_eq!(
        store.history("f1").unwrap()[0]
            .spec
            .model_version
            .as_deref(),
        Some("fake/any@111")
    );

    // Another version of the model, the same evidence: the judgment stands,
    // and the finding records the version it was last decided by.
    let mut again = observed("f1", "design/a", "a.go");
    again.model_version = Some("fake/any@222".to_owned());
    store.record(&run(vec![again])).unwrap();
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));
    assert_eq!(
        store.finding("f1").unwrap().model_version.as_deref(),
        Some("fake/any@222")
    );
}

#[test]
fn a_standing_stands_while_it_neither_expired_nor_was_refused() {
    assert!(Standing::Suppressed.stands());
    assert!(Standing::Judged.stands());
    assert!(!Standing::RuleChanged.stands());
    assert!(!Standing::EvidenceChanged.stands());
    assert!(!Standing::Unsuppressible.stands());
}

#[test]
fn a_judgment_applies_to_a_finding_the_store_never_recorded_and_expires_against_the_current_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut author = Store::open(dir.path()).unwrap();
    author
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    judge(&mut author, "f1", Judgment::Pass);
    drop(author);

    // A clone: the log, no cache, and no finding recorded.
    let clone = tempfile::tempdir().unwrap();
    fs::create_dir_all(clone.path().join(".lighthouse")).unwrap();
    fs::write(Store::log_path_in(clone.path()), log_of(dir.path())).unwrap();
    let store = Store::open(clone.path()).unwrap();
    assert!(store.list(&all()).unwrap().is_empty(), "no findings yet");

    let same = observed("f1", "design/a", "a.go");
    assert_eq!(standing_for(&store, &same), Some(Standing::Suppressed));
    let mut grown = same.clone();
    grown.evidence = json!({ "fan_out": 20 });
    assert_eq!(
        standing_for(&store, &grown),
        Some(Standing::EvidenceChanged)
    );
    let mut reworded = same.clone();
    reworded.meaning_version = Some("sem2".to_owned());
    assert_eq!(standing_for(&store, &reworded), Some(Standing::RuleChanged));
    let mut error = same;
    error.authored_severity = Severity::Error;
    assert_eq!(standing_for(&store, &error), Some(Standing::Unsuppressible));
}

#[test]
fn a_person_outranks_an_agent_on_the_same_subject_and_meaning_version() {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    let mut person = review("f1", Judgment::Fail);
    person.suppress = Some("named policy".to_owned());
    person.attribution.kind = lighthouse_model::AgentKind::Person;
    person.attribution.id = Some("ana".to_owned());
    store.resolve(&person, stamp("sem1")).unwrap();
    thread::sleep(std::time::Duration::from_millis(2));

    judge(&mut store, "f1", Judgment::Pass);

    let history = store.history("f1").unwrap();
    assert_eq!(history.len(), 2, "the weaker judgment is recorded");
    let finding = store.finding("f1").unwrap();
    assert_eq!(finding.judgment, Some(Judgment::Fail), "and does not stand");
    assert_eq!(finding.justification.as_deref(), Some("named policy"));
    assert_eq!(finding.standing, Some(Standing::Suppressed));

    // Equal strength: the later one stands.
    let mut later = review("f1", Judgment::NotApplicable);
    later.attribution.kind = lighthouse_model::AgentKind::Person;
    store.resolve(&later, stamp("sem1")).unwrap();
    assert_eq!(
        store.finding("f1").unwrap().judgment,
        Some(Judgment::NotApplicable)
    );

    // Under another meaning version a person's judgment does not outrank an
    // agent's: the agent's was given under the current one.
    let mut reworded = observed("f1", "design/a", "a.go");
    reworded.meaning_version = Some("sem2".to_owned());
    store.record(&run(vec![reworded])).unwrap();
    store
        .resolve(&review("f1", Judgment::Pass), stamp("sem2"))
        .unwrap();
    assert_eq!(store.finding("f1").unwrap().judgment, Some(Judgment::Pass));
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));
}

#[test]
fn a_suppression_needs_a_justification() {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    for blank in ["", "   ", "\t\n"] {
        let mut input = review("f1", Judgment::Fail);
        input.suppress = Some(blank.to_owned());
        let error = store.resolve(&input, stamp("sem1")).unwrap_err();
        assert!(matches!(error, Error::EmptyJustification), "{blank:?}");
    }
    assert!(store.history("f1").unwrap().is_empty());
}
