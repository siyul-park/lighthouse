//! Findings, runs and verdicts for the store tests.

use std::thread;

use lighthouse_model::{Reason, ReviewerKind, Severity, Verdict};
use lighthouse_store::{NewReview, Observed, Run, Stamp, Standing, Store, Unchecked};
use serde_json::json;

pub fn observed(fingerprint: &str, rule: &str, path: &str) -> Observed {
    Observed {
        fingerprint: fingerprint.to_owned(),
        legacy_fingerprints: Vec::new(),
        rule_id: rule.to_owned(),
        decision_uid: None,
        severity: Severity::Info,
        authored_severity: "info".to_owned(),
        path: path.to_owned(),
        locator: json!({ "span": { "start": { "line": 1 } } }),
        symbol: Some(format!("m::{fingerprint}#function")),
        message: format!("message of {fingerprint}"),
        evidence: json!({ "fan_out": 14 }),
        facts: json!({ "language": "go", "callers": 2 }),
        options: json!({ "max": 3 }),
        rule_version: Some("sem1".to_owned()),
        legacy_rule_version: None,
        check_revision: None,
        decision_hash: Some("full1".to_owned()),
    }
}

pub fn run(observed: Vec<Observed>) -> Run {
    Run {
        observed,
        reported: vec![String::new()],
        rules: vec!["design/a".to_owned(), "design/b".to_owned()],
        configured: vec!["design/a".to_owned(), "design/b".to_owned()],
        unchecked: Unchecked::Nothing,
        commit: Some("abc123".to_owned()),
        dirty: true,
        catalog_version: Some("cat1".to_owned()),
        lighthouse_version: "0.1.0".to_owned(),
    }
}

pub fn review(fingerprint: &str, verdict: Verdict, reason: Reason) -> NewReview {
    NewReview {
        fingerprint: fingerprint.to_owned(),
        verdict,
        reason,
        reason_text: None,
        reviewer_kind: ReviewerKind::Agent,
        reviewer_id: Some("claude".to_owned()),
        commit: Some("def456".to_owned()),
        expect_seen: None,
        lighthouse_version: "0.1.0".to_owned(),
    }
}

pub fn stamp(version: &str) -> impl Fn(&lighthouse_store::FindingRecord) -> Stamp + '_ {
    move |_| Stamp {
        decision_uid: None,
        rule_version: Some(version.to_owned()),
        check_revision: Some("chk1".to_owned()),
        decision_hash: Some("full1".to_owned()),
        catalog_version: Some("cat1".to_owned()),
        scope: Some("symbol".to_owned()),
    }
}

/// Records a verdict stamped with the rule version `sem1`. The latest verdict
/// is the one with the latest millisecond, so each waits for the next one.
pub fn judge(store: &mut Store, fingerprint: &str, verdict: Verdict, reason: Reason) {
    store
        .resolve(&review(fingerprint, verdict, reason), stamp("sem1"))
        .unwrap();
    thread::sleep(std::time::Duration::from_millis(2));
}

pub fn memory_with(findings: &[Observed]) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store.record(&run(findings.to_vec())).unwrap();
    store
}

pub fn standing_of(store: &Store, fingerprint: &str) -> Option<Standing> {
    store
        .standings()
        .unwrap()
        .get(fingerprint)
        .map(|j| j.standing)
}

/// The text of the decision log of the project at `dir`; empty if it has none.
pub fn log_of(dir: &std::path::Path) -> String {
    std::fs::read_to_string(Store::log_path_in(dir)).unwrap_or_default()
}
