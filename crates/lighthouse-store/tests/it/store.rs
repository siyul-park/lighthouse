use std::{fs, thread};

use crate::support::*;
use lighthouse_model::{Diagnostic, Fingerprint, Label, Position, Reason, Severity, Span, Verdict};
use lighthouse_store::{
    Error, Filter, Observed, Run, RunSummary, Standing, State, StatusFilter, Store, Unchecked,
};
use rusqlite::Connection;
use serde_json::json;

fn all() -> Filter {
    Filter {
        rule: None,
        status: StatusFilter::All,
    }
}

fn status(status: StatusFilter) -> Filter {
    Filter { rule: None, status }
}

fn fingerprints(store: &Store, filter: &Filter) -> Vec<String> {
    store
        .list(filter)
        .unwrap()
        .into_iter()
        .map(|f| f.fingerprint)
        .collect()
}

#[test]
fn store_opens_a_migrated_cache_and_refuses_a_newer_one() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Store::path_in(dir.path()).ends_with(".lighthouse/lighthouse.db"));
    assert!(Store::log_path_in(dir.path()).ends_with(".lighthouse/decisions.jsonl"));
    assert!(Store::open_existing(dir.path()).unwrap().is_none());

    let first = Store::open(dir.path()).unwrap();
    assert_eq!(first.schema_version().unwrap(), 7);
    drop(first);
    let again = Store::open_existing(dir.path()).unwrap().unwrap();
    assert_eq!(again.schema_version().unwrap(), 7);
    drop(again);

    Connection::open(Store::path_in(dir.path()))
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    let error = Store::open(dir.path()).err().unwrap();
    assert!(matches!(
        error,
        Error::NewerSchema {
            found: 99,
            supported: 7
        }
    ));
}

#[test]
fn a_garbage_cache_is_named_and_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = Store::path_in(dir.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "this is not a database, it is only a long enough line of text ".repeat(40),
    )
    .unwrap();
    let error = Store::open(dir.path()).err().unwrap();
    assert!(matches!(error, Error::Corrupt { .. }), "{error}");
    let message = error.to_string();
    assert!(message.contains(&path.display().to_string()), "{message}");
    assert!(message.contains("nothing was deleted"), "{message}");
    assert!(path.is_file());
}

#[test]
fn processes_opening_a_fresh_cache_together_all_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let root = root.clone();
            thread::spawn(move || Store::open(&root).unwrap().schema_version().unwrap())
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), 7);
    }
}

#[test]
fn two_stores_on_one_cache_can_record_and_resolve_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut seed = Store::open(dir.path()).unwrap();
    seed.record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    drop(seed);

    let root = dir.path().to_owned();
    let recorder = {
        let root = root.clone();
        thread::spawn(move || {
            let mut store = Store::open(&root).unwrap();
            for _ in 0..25 {
                store
                    .record(&run(vec![observed("f1", "design/a", "a.go")]))
                    .unwrap();
            }
        })
    };
    let judge = thread::spawn(move || {
        let mut store = Store::open(&root).unwrap();
        for _ in 0..25 {
            store
                .resolve(
                    &review("f1", Verdict::Deferred, Reason::Unspecified),
                    stamp("sem1"),
                )
                .unwrap();
        }
    });
    recorder.join().unwrap();
    judge.join().unwrap();
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.history("f1").unwrap().len(), 25);
}

#[test]
fn record_inserts_refreshes_and_reopens_findings() {
    let mut store = Store::open_in_memory().unwrap();
    let mut finding = observed("f1", "design/a", "a.go");
    let summary = store.record(&run(vec![finding.clone()])).unwrap();
    assert_eq!(
        summary,
        RunSummary {
            opened: 1,
            reopened: 0,
            resolved: 0,
            deactivated: 0,
            rewritten: 0
        }
    );
    let first = store.finding("f1").unwrap();
    assert_eq!(first.message, "message of f1");
    assert_eq!(first.first_seen, first.last_seen);
    assert_eq!(first.commit.as_deref(), Some("abc123"));
    assert_eq!(first.dirty, Some(true));
    assert_eq!(first.options["max"], 3);
    assert_eq!(first.authored_severity.as_deref(), Some("info"));
    assert_eq!(first.lighthouse_version.as_deref(), Some("0.1.0"));
    assert_eq!(first.catalog_version.as_deref(), Some("cat1"));
    assert_eq!(first.state(), State::Open);

    finding.message = "newer".to_owned();
    finding.facts = json!({ "language": "go", "callers": 9 });
    store.record(&run(vec![finding.clone()])).unwrap();
    let second = store.finding("f1").unwrap();
    assert_eq!(second.message, "newer");
    assert_eq!(second.facts["callers"], 9);
    assert_eq!(second.first_seen, first.first_seen);
    assert!(second.last_seen >= first.last_seen);

    let gone = store.record(&run(vec![])).unwrap();
    assert_eq!(gone.resolved, 1);
    let resolved = store.finding("f1").unwrap();
    assert!(resolved.resolved_at.is_some());
    assert_eq!(resolved.state(), State::Resolved);

    let back = store.record(&run(vec![finding])).unwrap();
    assert_eq!(back.reopened, 1);
    let reopened = store.finding("f1").unwrap();
    assert_eq!(reopened.resolved_at, None);
    assert_eq!(reopened.reopened, 1);
}

#[test]
fn record_resolves_only_inside_the_reported_scope() {
    let mut store = memory_with(&[
        observed("in", "design/a", "src/a.go"),
        observed("outside", "design/a", "docs/b.go"),
        observed("other-rule", "design/b", "src/c.go"),
    ]);
    let partial = Run {
        reported: vec!["src".to_owned()],
        rules: vec!["design/a".to_owned()],
        ..run(vec![])
    };
    assert_eq!(store.record(&partial).unwrap().resolved, 1);
    assert!(store.finding("in").unwrap().resolved_at.is_some());
    assert_eq!(store.finding("outside").unwrap().resolved_at, None);
    assert_eq!(store.finding("other-rule").unwrap().resolved_at, None);

    let nothing_reported = Run {
        reported: vec![],
        ..partial
    };
    assert_eq!(store.record(&nothing_reported).unwrap().resolved, 0);
    assert_eq!(store.finding("outside").unwrap().resolved_at, None);
}

#[test]
fn record_never_resolves_what_the_run_could_not_check() {
    let findings = [
        observed("seen", "design/a", "pkg/a.go"),
        observed("same-dir", "design/a", "pkg/b.go"),
        observed("elsewhere", "design/a", "other/c.go"),
    ];
    let mut store = memory_with(&findings);
    let seen = observed("seen", "design/a", "pkg/a.go");

    let everything = Run {
        unchecked: Unchecked::Everything,
        ..run(vec![seen.clone()])
    };
    assert_eq!(store.record(&everything).unwrap().resolved, 0);

    let one_directory = Run {
        unchecked: Unchecked::Paths(vec!["pkg".to_owned()]),
        ..run(vec![seen])
    };
    let summary = store.record(&one_directory).unwrap();
    assert_eq!(
        summary.resolved, 1,
        "only the finding outside pkg is resolved"
    );
    assert_eq!(store.finding("same-dir").unwrap().resolved_at, None);
    assert!(store.finding("elsewhere").unwrap().resolved_at.is_some());
}

#[test]
fn findings_of_rules_no_longer_configured_go_inactive_and_come_back() {
    let mut store = memory_with(&[
        observed("kept", "design/a", "a.go"),
        observed("dropped", "design/b", "b.go"),
    ]);
    let only_a = Run {
        configured: vec!["design/a".to_owned()],
        rules: vec!["design/a".to_owned()],
        ..run(vec![observed("kept", "design/a", "a.go")])
    };
    assert_eq!(store.record(&only_a).unwrap().deactivated, 1);
    let dropped = store.finding("dropped").unwrap();
    assert!(dropped.inactive_at.is_some());
    assert_eq!(dropped.state(), State::Inactive);
    assert_eq!(fingerprints(&store, &status(StatusFilter::Open)), ["kept"]);
    assert_eq!(
        fingerprints(&store, &status(StatusFilter::Inactive)),
        ["dropped"]
    );

    store
        .record(&run(vec![
            observed("kept", "design/a", "a.go"),
            observed("dropped", "design/b", "b.go"),
        ]))
        .unwrap();
    assert_eq!(store.finding("dropped").unwrap().inactive_at, None);
    assert_eq!(
        fingerprints(&store, &status(StatusFilter::Open)),
        ["kept", "dropped"]
    );
}

#[test]
fn finding_expands_unique_prefixes_and_rejects_the_rest() {
    let store = memory_with(&[
        observed("abc1", "design/a", "a.go"),
        observed("abc2", "design/a", "b.go"),
        observed("xyz", "design/a", "c.go"),
    ]);
    assert_eq!(store.finding("abc1").unwrap().fingerprint, "abc1");
    assert_eq!(store.finding("xy").unwrap().fingerprint, "xyz");
    let Err(Error::AmbiguousFinding { candidates, .. }) = store.finding("abc") else {
        panic!("abc is ambiguous");
    };
    assert_eq!(candidates, ["abc1", "abc2"]);
    let message = store.finding("abc").unwrap_err().to_string();
    assert!(
        message.contains("longer prefix") && message.contains("abc2"),
        "{message}"
    );
    assert!(matches!(
        store.finding("nope"),
        Err(Error::UnknownFinding(_))
    ));
}

#[test]
fn resolve_freezes_the_finding_as_it_was_last_seen() {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    let mut input = review("f1", Verdict::Rejected, Reason::IntentionalException);
    input.reason_text = Some("hot path".to_owned());
    let resolved = store.resolve(&input, stamp("sem1")).unwrap();
    let event = &resolved.event;
    assert_eq!(event.fingerprint, "f1");
    assert_eq!(event.rule_id, "design/a");
    assert_eq!(event.rule_version.as_deref(), Some("sem1"));
    assert_eq!(event.decision_hash.as_deref(), Some("full1"));
    assert_eq!(event.catalog_version.as_deref(), Some("cat1"));
    assert_eq!(event.lighthouse_version.as_deref(), Some("0.1.0"));
    assert_eq!(event.language.as_deref(), Some("go"));
    assert_eq!(event.scope.as_deref(), Some("symbol"));
    assert_eq!(event.commit.as_deref(), Some("def456"));
    assert_eq!(event.reviewer_id.as_deref(), Some("claude"));
    assert_eq!(event.label(), Label::Separate);
    assert_eq!(event.id.len(), 32);
    assert_eq!(resolved.finding.fingerprint, "f1");

    let snapshot = &event.snapshot;
    assert_eq!(snapshot["v"], 2);
    assert_eq!(snapshot["evidence"]["fan_out"], 14);
    assert_eq!(event.evidence()["fan_out"], 14);
    assert_eq!(snapshot["facts"]["callers"], 2);
    assert_eq!(snapshot["options"]["max"], 3);
    assert_eq!(snapshot["severity"], "info");
    assert_eq!(snapshot["authored"], "info");
    assert_eq!(snapshot["seenAt"], resolved.finding.last_seen);
    assert_eq!(snapshot["commit"], "abc123");
    assert_eq!(snapshot["dirty"], true);
    assert_eq!(snapshot["lighthouseVersion"], "0.1.0");
    assert!(snapshot["patternFingerprint"].is_null());

    let mut changed = observed("f1", "design/a", "a.go");
    changed.facts = json!({ "language": "go", "callers": 50 });
    store.record(&run(vec![changed])).unwrap();
    judge(&mut store, "f1", Verdict::Deferred, Reason::Unspecified);
    let history = store.history("f1").unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].snapshot["facts"]["callers"], 2);
    assert_eq!(history[1].snapshot["facts"]["callers"], 50);
}

#[test]
fn resolve_validates_reasons_findings_and_the_sighting() {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    for (verdict, reason) in [
        (Verdict::Rejected, Reason::Unspecified),
        (Verdict::Rejected, Reason::Fixed),
        (Verdict::Confirmed, Reason::FalsePositive),
        (Verdict::Deferred, Reason::AcceptedDebt),
    ] {
        let error = store
            .resolve(&review("f1", verdict, reason), stamp("sem1"))
            .unwrap_err();
        assert!(matches!(error, Error::Reason(_)), "{verdict} {reason}");
    }
    assert!(store.history("f1").unwrap().is_empty());
    let unknown = store.resolve(
        &review("zz", Verdict::Confirmed, Reason::Fixed),
        stamp("sem1"),
    );
    assert!(matches!(unknown, Err(Error::UnknownFinding(_))));

    let seen = store.finding("f1").unwrap().last_seen;
    let mut stale = review("f1", Verdict::Confirmed, Reason::Fixed);
    stale.expect_seen = Some("2000-01-01T00:00:00.000Z".to_owned());
    assert!(matches!(
        store.resolve(&stale, stamp("sem1")),
        Err(Error::Changed { .. })
    ));
    let mut current = review("f1", Verdict::Confirmed, Reason::Fixed);
    current.expect_seen = Some(seen);
    store.resolve(&current, stamp("sem1")).unwrap();
}

#[test]
fn review_log_is_append_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    judge(&mut store, "f1", Verdict::Confirmed, Reason::Fixed);

    let raw = Connection::open(Store::path_in(dir.path())).unwrap();
    let update = raw.execute("UPDATE review_events SET verdict = 'rejected'", []);
    assert!(update.unwrap_err().to_string().contains("append-only"));
    let delete = raw.execute("DELETE FROM review_events", []);
    assert!(delete.unwrap_err().to_string().contains("append-only"));
    assert_eq!(store.history("f1").unwrap().len(), 1);
}

#[test]
fn standings_follow_the_latest_verdict() {
    let mut store = memory_with(&[
        observed("rejected", "design/a", "a.go"),
        observed("confirmed", "design/a", "b.go"),
        observed("reconsidered", "design/a", "c.go"),
        observed("broad", "design/a", "d.go"),
        observed("unreviewed", "design/a", "e.go"),
    ]);
    judge(
        &mut store,
        "rejected",
        Verdict::Rejected,
        Reason::FalsePositive,
    );
    judge(
        &mut store,
        "confirmed",
        Verdict::Confirmed,
        Reason::AcceptedDebt,
    );
    judge(
        &mut store,
        "reconsidered",
        Verdict::Rejected,
        Reason::ProjectAllowed,
    );
    judge(
        &mut store,
        "broad",
        Verdict::Rejected,
        Reason::ScopeTooBroad,
    );
    let standings = store.standings().unwrap();
    let keys: Vec<_> = standings.keys().map(String::as_str).collect();
    assert_eq!(keys, ["broad", "reconsidered", "rejected"]);
    assert!(
        standings
            .values()
            .all(|j| j.standing == Standing::Suppressed)
    );
    assert_eq!(standings["rejected"].reason, Reason::FalsePositive);

    judge(
        &mut store,
        "reconsidered",
        Verdict::Deferred,
        Reason::Unspecified,
    );
    assert_eq!(standing_of(&store, "reconsidered"), None);
    assert_eq!(store.finding("reconsidered").unwrap().state(), State::Open);
    assert_eq!(
        store.finding("rejected").unwrap().state(),
        State::Suppressed
    );
}

#[test]
fn a_verdict_expires_when_the_rule_or_the_evidence_changes() {
    let mut store = memory_with(&[
        observed("rule", "design/a", "a.go"),
        observed("evidence", "design/a", "b.go"),
        observed("same", "design/a", "c.go"),
    ]);
    for fingerprint in ["rule", "evidence", "same"] {
        judge(
            &mut store,
            fingerprint,
            Verdict::Rejected,
            Reason::IntentionalException,
        );
    }
    assert_eq!(standing_of(&store, "same"), Some(Standing::Suppressed));

    let mut rule = observed("rule", "design/a", "a.go");
    rule.rule_version = Some("sem2".to_owned());
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
fn evidence_that_only_reflows_does_not_expire_a_verdict() {
    let mut first = observed("f1", "design/a", "a.go");
    first.evidence = json!({ "callee": "a  b\n c", "n": 1 });
    let mut store = memory_with(&[first]);
    judge(&mut store, "f1", Verdict::Rejected, Reason::FalsePositive);
    let mut reflowed = observed("f1", "design/a", "a.go");
    reflowed.evidence = json!({ "n": 1, "callee": "a b c" });
    store.record(&run(vec![reflowed])).unwrap();
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));
}

#[test]
fn a_verdict_suppresses_by_the_authored_severity_of_the_finding_not_by_its_reported_one() {
    let mut error = observed("f1", "design/a", "a.go");
    error.severity = Severity::Error;
    error.authored_severity = "warn".to_owned();
    let mut store = memory_with(&[error]);
    judge(&mut store, "f1", Verdict::Rejected, Reason::FalsePositive);
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));
    assert!(store.finding("f1").unwrap().needs_verdict());
}

#[test]
fn definitive_findings_are_never_suppressed_by_a_verdict() {
    let mut error = observed("f1", "design/a", "a.go");
    error.severity = Severity::Warn;
    error.authored_severity = "error".to_owned();
    let mut store = memory_with(&[error]);
    judge(
        &mut store,
        "f1",
        Verdict::Rejected,
        Reason::IntentionalException,
    );
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Unsuppressible));
    assert_eq!(
        store.history("f1").unwrap().len(),
        1,
        "the verdict is still recorded"
    );
    assert_eq!(fingerprints(&store, &status(StatusFilter::Open)), ["f1"]);
    assert!(fingerprints(&store, &status(StatusFilter::Suppressed)).is_empty());
}

#[test]
fn narrowing_lists_the_findings_rejected_as_too_broad() {
    let mut store = memory_with(&[
        observed("broad", "design/a", "a.go"),
        observed("wrong", "design/a", "b.go"),
    ]);
    judge(
        &mut store,
        "broad",
        Verdict::Rejected,
        Reason::ScopeTooBroad,
    );
    judge(
        &mut store,
        "wrong",
        Verdict::Rejected,
        Reason::FalsePositive,
    );
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
fn list_filters_by_rule_and_status() {
    let mut store = memory_with(&[
        observed("a1", "design/a", "a.go"),
        observed("a2", "design/a", "b.go"),
        observed("b1", "design/b", "c.go"),
        observed("gone", "design/b", "d.go"),
    ]);
    judge(&mut store, "a2", Verdict::Rejected, Reason::NotWorthFixing);
    let again = run(vec![
        observed("a1", "design/a", "a.go"),
        observed("a2", "design/a", "b.go"),
        observed("b1", "design/b", "c.go"),
    ]);
    store.record(&again).unwrap();

    assert_eq!(fingerprints(&store, &all()), ["a1", "a2", "b1", "gone"]);
    assert_eq!(
        fingerprints(&store, &status(StatusFilter::Open)),
        ["a1", "b1"]
    );
    assert_eq!(
        fingerprints(&store, &status(StatusFilter::Suppressed)),
        ["a2"]
    );
    assert_eq!(
        fingerprints(&store, &status(StatusFilter::Resolved)),
        ["gone"]
    );
    let rule = Filter {
        rule: Some("design/b".to_owned()),
        status: StatusFilter::All,
    };
    assert_eq!(fingerprints(&store, &rule), ["b1", "gone"]);
    let listed = store.list(&status(StatusFilter::Suppressed)).unwrap();
    let latest = listed[0].review.unwrap();
    assert_eq!(latest.verdict, Verdict::Rejected);
    assert_eq!(latest.reason, Reason::NotWorthFixing);
    assert_eq!(listed[0].standing, Some(Standing::Suppressed));
}

#[test]
fn prune_removes_only_unreviewed_findings_that_are_resolved_or_inactive() {
    let mut store = memory_with(&[
        observed("open", "design/a", "a.go"),
        observed("resolved", "design/a", "b.go"),
        observed("reviewed", "design/a", "c.go"),
    ]);
    judge(&mut store, "reviewed", Verdict::Confirmed, Reason::Fixed);
    store
        .record(&run(vec![observed("open", "design/a", "a.go")]))
        .unwrap();
    assert_eq!(store.prune(Some(30)).unwrap(), 0, "too recent");
    assert_eq!(store.prune(None).unwrap(), 1);
    assert_eq!(fingerprints(&store, &all()), ["open", "reviewed"]);
    assert_eq!(store.history("reviewed").unwrap().len(), 1);
}

#[test]
fn observed_records_a_diagnostic_and_digests_its_evidence() {
    let span = Span {
        start: Position { line: 3, col: 1 },
        end: Position { line: 9, col: 2 },
    };
    let mut diagnostic = Diagnostic::new(
        "design/a",
        Severity::Warn,
        "too big",
        "src/a.go",
        span,
        Fingerprint::of("design/a", "m::f#function", ""),
    );
    diagnostic.symbol = Some("m::f#function".to_owned());
    diagnostic.evidence = json!({ "n": 1, "name": "a   b" });
    let record = Observed::from_diagnostic(&diagnostic, json!({ "language": "go" }));
    assert_eq!(record.fingerprint, diagnostic.fingerprint.as_str());
    assert_eq!(record.path, "src/a.go");
    assert_eq!(record.locator["span"]["start"]["line"], 3);
    assert_eq!(record.symbol.as_deref(), Some("m::f#function"));
    assert_eq!(record.severity, Severity::Warn);

    let mut spaced = record.clone();
    spaced.evidence = json!({ "name": "a b", "n": 1 });
    assert_eq!(spaced.evidence_digest(), record.evidence_digest());
    spaced.evidence = json!({ "name": "a b", "n": 2 });
    assert_ne!(spaced.evidence_digest(), record.evidence_digest());
}

/// The schema of version 1, as the first release wrote it.
const V1_SCHEMA: &str = "
CREATE TABLE findings (
    fingerprint TEXT PRIMARY KEY, rule_id TEXT NOT NULL, severity TEXT NOT NULL,
    path TEXT NOT NULL, locator TEXT NOT NULL, symbol TEXT, first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL, resolved_at TEXT, reopened INTEGER NOT NULL DEFAULT 0,
    last_message TEXT NOT NULL, last_evidence TEXT NOT NULL, last_facts TEXT NOT NULL
);
CREATE TABLE review_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fingerprint TEXT NOT NULL REFERENCES findings (fingerprint),
    rule_id TEXT NOT NULL, rule_version TEXT, catalog_version TEXT, pattern_fingerprint TEXT,
    verdict TEXT NOT NULL CHECK (verdict IN ('confirmed', 'rejected', 'deferred')),
    reason_code TEXT NOT NULL, reason_text TEXT,
    reviewer_kind TEXT NOT NULL CHECK (reviewer_kind IN ('agent', 'human')),
    reviewer_id TEXT, language TEXT, scope TEXT, evidence TEXT NOT NULL,
    feature_snapshot TEXT NOT NULL, git_commit TEXT, timestamp TEXT NOT NULL
);
CREATE TRIGGER review_events_append_only_update BEFORE UPDATE ON review_events
BEGIN SELECT RAISE(ABORT, 'review_events is append-only'); END;
CREATE TRIGGER review_events_append_only_delete BEFORE DELETE ON review_events
BEGIN SELECT RAISE(ABORT, 'review_events is append-only'); END;
CREATE VIEW latest_verdicts AS
SELECT fingerprint, id AS event_id, verdict, reason_code FROM review_events e
WHERE id = (SELECT MAX(id) FROM review_events WHERE fingerprint = e.fingerprint);
CREATE VIEW suppressions AS
SELECT fingerprint, event_id, reason_code, (reason_code = 'scope-too-broad') AS narrowing
FROM latest_verdicts WHERE verdict = 'rejected';
CREATE VIEW finding_states AS
SELECT f.*, l.verdict, l.reason_code,
       EXISTS (SELECT 1 FROM suppressions s WHERE s.fingerprint = f.fingerprint) AS suppressed
FROM findings f LEFT JOIN latest_verdicts l ON l.fingerprint = f.fingerprint;
";

#[test]
fn migration_rewrites_the_append_only_table_and_exports_old_verdicts_to_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = Store::path_in(dir.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let old = Connection::open(&path).unwrap();
        old.execute_batch(V1_SCHEMA).unwrap();
        old.execute(
            "INSERT INTO findings VALUES ('f1', 'design/a', 'review', 'a.go', '{}', NULL, \
             '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z', NULL, 0, 'm', '{\"n\":1}', '{}')",
            [],
        )
        .unwrap();
        old.execute(
            "INSERT INTO review_events (fingerprint, rule_id, rule_version, verdict, reason_code, \
             reviewer_kind, evidence, feature_snapshot, timestamp) VALUES ('f1', 'design/a', 'pattern-hash', \
             'rejected', 'false-positive', 'human', '{\"n\":1}', '{\"evidence\":{\"n\":1}}', '2026-01-02T00:00:00.000Z')",
            [],
        )
        .unwrap();
        old.pragma_update(None, "user_version", 1).unwrap();
    }

    let mut store = Store::open(dir.path()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 7);
    let history = store.history("f1").unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].id.starts_with("legacy-"));
    assert_eq!(history[0].decision_hash.as_deref(), Some("pattern-hash"));
    assert_eq!(history[0].rule_version, None);
    assert_eq!(history[0].evidence()["n"], 1);
    assert_eq!(log_of(dir.path()).lines().count(), 1, "exported once");

    let migrated = store.finding("f1").unwrap();
    assert_eq!(migrated.severity, "review");
    store
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    assert_eq!(
        standing_of(&store, "f1"),
        Some(Standing::Suppressed),
        "an old verdict matches any version"
    );
    drop(store);
    assert_eq!(
        Store::open(dir.path())
            .unwrap()
            .history("f1")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        log_of(dir.path()).lines().count(),
        1,
        "and is not exported again"
    );

    let raw = Connection::open(&path).unwrap();
    let update = raw.execute("UPDATE review_events SET verdict = 'confirmed'", []);
    assert!(
        update.unwrap_err().to_string().contains("append-only"),
        "triggers are back"
    );
}

#[test]
fn as_str_names_each_standing() {
    let names = [
        (Standing::Suppressed, "suppressed"),
        (Standing::RuleChanged, "rule-changed"),
        (Standing::EvidenceChanged, "evidence-changed"),
        (Standing::Unsuppressible, "unsuppressible"),
    ];
    for (standing, name) in names {
        assert_eq!(standing.as_str(), name);
    }
}

#[test]
fn a_verdict_recorded_under_the_semantic_version_of_before_still_applies_while_the_decision_is_unchanged()
 {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    judge(
        &mut store,
        "f1",
        Verdict::Rejected,
        Reason::IntentionalException,
    );
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));

    // The decision is now identified by a new hash, and remembers the old one.
    let mut migrated = observed("f1", "design/a", "a.go");
    migrated.rule_version = Some("sem-new".to_owned());
    migrated.legacy_rule_version = Some("sem1".to_owned());
    store.record(&run(vec![migrated])).unwrap();
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));

    // Once the decision changes, the old verdict goes with its old hash.
    let mut edited = observed("f1", "design/a", "a.go");
    edited.rule_version = Some("sem-newer".to_owned());
    edited.legacy_rule_version = Some("sem-edited".to_owned());
    store.record(&run(vec![edited])).unwrap();
    assert_eq!(standing_of(&store, "f1"), Some(Standing::RuleChanged));
}

#[test]
fn a_finding_asks_for_a_verdict_when_its_decision_did_not_author_an_error() {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    let finding = |store: &Store| store.finding("f1").unwrap();
    assert!(finding(&store).needs_verdict(), "authored info");

    let mut definitive = observed("f1", "design/a", "a.go");
    definitive.authored_severity = "error".to_owned();
    store.record(&run(vec![definitive])).unwrap();
    assert!(!finding(&store).needs_verdict());

    let mut review = observed("f1", "design/a", "a.go");
    review.authored_severity = "warn".to_owned();
    review.severity = Severity::Error;
    store.record(&run(vec![review])).unwrap();
    assert!(
        finding(&store).needs_verdict(),
        "whatever the reported level"
    );
}

#[test]
fn changing_only_how_a_decision_is_checked_keeps_its_verdicts_and_changing_what_it_means_expires_them()
 {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    judge(&mut store, "f1", Verdict::Rejected, Reason::FalsePositive);

    // Same meaning, another check: the verdict stands.
    let mut rechecked = observed("f1", "design/a", "a.go");
    rechecked.check_revision = Some("chk2".to_owned());
    store.record(&run(vec![rechecked])).unwrap();
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));

    // Another meaning: the verdict is asked again.
    let mut reworded = observed("f1", "design/a", "a.go");
    reworded.rule_version = Some("sem2".to_owned());
    store.record(&run(vec![reworded])).unwrap();
    assert_eq!(standing_of(&store, "f1"), Some(Standing::RuleChanged));
}

#[test]
fn every_earlier_version_a_decision_lists_honors_the_verdicts_recorded_under_it() {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    judge(&mut store, "f1", Verdict::Rejected, Reason::FalsePositive);
    let mut moved = observed("f1", "design/a", "a.go");
    moved.rule_version = Some("meaning".to_owned());
    moved.legacy_rule_version = Some("previous,sem1,oldest".to_owned());
    store.record(&run(vec![moved])).unwrap();
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));
}

#[test]
fn the_migration_turns_recorded_tiers_into_authored_severities() {
    let dir = tempfile::tempdir().unwrap();
    let path = Store::path_in(dir.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let mut store = Store::open(dir.path()).unwrap();
        store
            .record(&run(vec![
                observed("m", "design/a", "a.go"),
                observed("h", "design/a", "b.go"),
                observed("j", "design/a", "c.go"),
            ]))
            .unwrap();
    }
    let raw = Connection::open(&path).unwrap();
    // Put the database back where V5 left it, with tiers.
    raw.execute_batch(
        "DROP VIEW finding_states;
         DROP VIEW standings;
         DROP VIEW latest_verdicts;
         DROP INDEX review_events_subject;
         DROP TRIGGER review_events_append_only_update;
         DROP TABLE decision_uids;
         DROP TABLE fingerprint_rewrites;
         ALTER TABLE findings DROP COLUMN decision_uid;
         ALTER TABLE review_events DROP COLUMN decision_uid;
         ALTER TABLE review_events DROP COLUMN subject;
         CREATE TRIGGER review_events_append_only_update BEFORE UPDATE ON review_events
         BEGIN SELECT RAISE(ABORT, 'review_events is append-only'); END;
         CREATE VIEW latest_verdicts AS
         SELECT e.fingerprint, e.event_id, e.verdict, e.reason_code, e.rule_version, e.evidence_digest
         FROM review_events e;
         CREATE VIEW standings AS SELECT f.fingerprint FROM findings f;
         CREATE VIEW finding_states AS SELECT f.* FROM findings f;
         ALTER TABLE findings DROP COLUMN check_revision;
         ALTER TABLE review_events DROP COLUMN check_revision;
         ALTER TABLE findings RENAME COLUMN authored_severity TO tier;
         UPDATE findings SET tier = CASE fingerprint WHEN 'm' THEN 'mechanical' WHEN 'h' THEN 'heuristic' ELSE 'judgment' END;
         PRAGMA user_version = 5;",
    )
    .unwrap();
    drop(raw);
    let store = Store::open(dir.path()).unwrap();
    let authored = |fingerprint: &str| store.finding(fingerprint).unwrap().authored_severity;
    assert_eq!(authored("m").as_deref(), Some("error"));
    assert_eq!(authored("h").as_deref(), Some("warn"));
    assert_eq!(authored("j").as_deref(), Some("info"));
}
