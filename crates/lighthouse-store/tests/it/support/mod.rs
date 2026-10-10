//! Findings, runs and judgments for the store tests.

use std::thread;

use lighthouse_model::{AgentKind, Attribution, Judgment, Severity};
use lighthouse_store::{NewJudgment, Observed, Run, Stamp, Standing, Store, Unchecked};
use serde_json::json;

pub fn observed(fingerprint: &str, rule: &str, path: &str) -> Observed {
    Observed {
        fingerprint: fingerprint.to_owned(),
        rule_id: rule.to_owned(),
        decision_uid: None,
        severity: Severity::Info,
        authored_severity: Severity::Info,
        path: path.to_owned(),
        locator: json!({ "span": { "start": { "line": 1 } } }),
        symbol: Some(format!("m::{fingerprint}#function")),
        message: format!("message of {fingerprint}"),
        evidence: json!({ "fan_out": 14 }),
        facts: json!({ "language": "go", "callers": 2 }),
        options: json!({ "max": 3 }),
        meaning_version: Some("sem1".to_owned()),
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

pub fn review(fingerprint: &str, judgment: Judgment) -> NewJudgment {
    NewJudgment {
        fingerprint: fingerprint.to_owned(),
        judgment,
        reason: None,
        suppress: None,
        attribution: Attribution {
            kind: AgentKind::SoftwareAgent,
            id: Some("claude".to_owned()),
        },
        commit: Some("def456".to_owned()),
        expect_seen: None,
        lighthouse_version: "0.1.0".to_owned(),
    }
}

pub fn stamp(version: &str) -> impl Fn(&lighthouse_store::FindingRecord) -> Stamp + '_ {
    move |_| Stamp {
        decision_uid: None,
        meaning_version: Some(version.to_owned()),
        check_revision: Some("chk1".to_owned()),
        decision_hash: Some("full1".to_owned()),
        catalog_version: Some("cat1".to_owned()),
        scope: Some("symbol".to_owned()),
    }
}

/// Records a judgment stamped with the meaning version `sem1`. The latest
/// judgment is the one with the latest millisecond, so each waits for the
/// next one.
pub fn judge(store: &mut Store, fingerprint: &str, judgment: Judgment) {
    store
        .resolve(&review(fingerprint, judgment), stamp("sem1"))
        .unwrap();
    thread::sleep(std::time::Duration::from_millis(2));
}

/// Records a `fail` that is left in place on purpose.
pub fn judge_suppressed(store: &mut Store, fingerprint: &str, justification: &str) {
    let mut input = review(fingerprint, Judgment::Fail);
    input.suppress = Some(justification.to_owned());
    store.resolve(&input, stamp("sem1")).unwrap();
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
