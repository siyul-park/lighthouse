use lighthouse_model::{
    Diagnostic, Fingerprint, Label, Position, Reason, ReviewerKind, Severity, Span, Verdict,
};
use lighthouse_store::{Error, Filter, NewReview, Observed, Run, RunSummary, StatusFilter, Store};
use rusqlite::Connection;
use serde_json::json;

fn observed(fingerprint: &str, rule: &str, path: &str) -> Observed {
    Observed {
        fingerprint: fingerprint.to_owned(),
        rule_id: rule.to_owned(),
        severity: Severity::Review,
        path: path.to_owned(),
        locator: json!({ "span": { "line": 1 } }),
        symbol: Some(format!("m::{fingerprint}#function")),
        message: format!("message of {fingerprint}"),
        evidence: json!({ "fan_out": 14 }),
        facts: json!({ "language": "go", "callers": 2 }),
    }
}

fn run(observed: Vec<Observed>) -> Run {
    Run {
        observed,
        reported: vec![String::new()],
        rules: vec!["design/a".to_owned(), "design/b".to_owned()],
        complete: true,
    }
}

fn review(fingerprint: &str, verdict: Verdict, reason: Reason) -> NewReview {
    NewReview {
        fingerprint: fingerprint.to_owned(),
        verdict,
        reason,
        reason_text: None,
        reviewer_kind: ReviewerKind::Agent,
        reviewer_id: Some("claude".to_owned()),
        rule_version: Some("v1".to_owned()),
        catalog_version: Some("c1".to_owned()),
        pattern_fingerprint: None,
        scope: Some("symbol".to_owned()),
        commit: Some("abc123".to_owned()),
    }
}

fn all() -> Filter {
    Filter {
        rule: None,
        status: StatusFilter::All,
    }
}

fn open_with(findings: &[Observed]) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store.record(&run(findings.to_vec())).unwrap();
    store
}

#[test]
fn store_opens_a_migrated_database_and_refuses_a_newer_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = Store::path_in(dir.path());
    assert!(path.ends_with(".lighthouse/lighthouse.db"));
    assert!(Store::open_existing(&path).unwrap().is_none());

    let first = Store::open(&path).unwrap();
    assert_eq!(first.schema_version().unwrap(), 1);
    drop(first);
    let again = Store::open_existing(&path).unwrap().unwrap();
    assert_eq!(again.schema_version().unwrap(), 1);
    drop(again);

    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    let error = Store::open(&path).err().unwrap();
    assert!(matches!(
        error,
        Error::NewerSchema {
            found: 99,
            supported: 1
        }
    ));
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
            resolved: 0
        }
    );
    let first = store.finding("f1").unwrap();
    assert_eq!(first.message, "message of f1");
    assert_eq!(first.first_seen, first.last_seen);

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
    assert!(store.finding("f1").unwrap().resolved_at.is_some());

    let back = store.record(&run(vec![finding])).unwrap();
    assert_eq!(back.reopened, 1);
    let reopened = store.finding("f1").unwrap();
    assert_eq!(reopened.resolved_at, None);
    assert_eq!(reopened.reopened, 1);
}

#[test]
fn record_resolves_only_inside_the_reported_scope() {
    let mut store = open_with(&[
        observed("in", "design/a", "src/a.go"),
        observed("outside", "design/a", "docs/b.go"),
        observed("other-rule", "design/b", "src/c.go"),
    ]);
    let partial = Run {
        observed: vec![],
        reported: vec!["src".to_owned()],
        rules: vec!["design/a".to_owned()],
        complete: true,
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
fn record_never_resolves_when_the_analysis_was_incomplete() {
    let mut store = open_with(&[
        observed("seen", "design/a", "a.go"),
        observed("unseen", "design/a", "b.go"),
    ]);
    let incomplete = Run {
        complete: false,
        ..run(vec![observed("seen", "design/a", "a.go")])
    };
    let summary = store.record(&incomplete).unwrap();
    assert_eq!(summary.resolved, 0);
    assert_eq!(store.finding("unseen").unwrap().resolved_at, None);
}

#[test]
fn finding_expands_unique_prefixes_and_rejects_the_rest() {
    let store = open_with(&[
        observed("abc1", "design/a", "a.go"),
        observed("abc2", "design/a", "b.go"),
        observed("xyz", "design/a", "c.go"),
    ]);
    assert_eq!(store.finding("abc1").unwrap().fingerprint, "abc1");
    assert_eq!(store.finding("xy").unwrap().fingerprint, "xyz");
    assert!(matches!(
        store.finding("abc"),
        Err(Error::AmbiguousFinding(_))
    ));
    assert!(matches!(
        store.finding("nope"),
        Err(Error::UnknownFinding(_))
    ));
}

#[test]
fn resolve_appends_events_with_a_frozen_snapshot() {
    let mut store = open_with(&[observed("f1", "design/a", "a.go")]);
    let mut input = review("f1", Verdict::Rejected, Reason::IntentionalException);
    input.reason_text = Some("hot path".to_owned());
    let event = store.resolve(&input).unwrap();
    assert_eq!(event.fingerprint, "f1");
    assert_eq!(event.rule_id, "design/a");
    assert_eq!(event.language.as_deref(), Some("go"));
    assert_eq!(event.scope.as_deref(), Some("symbol"));
    assert_eq!(event.commit.as_deref(), Some("abc123"));
    assert_eq!(event.reviewer_id.as_deref(), Some("claude"));
    assert_eq!(event.evidence["fan_out"], 14);
    assert_eq!(event.feature_snapshot["facts"]["callers"], 2);
    assert_eq!(event.label(), Label::Separate);

    let mut changed = observed("f1", "design/a", "a.go");
    changed.facts = json!({ "language": "go", "callers": 50 });
    store.record(&run(vec![changed])).unwrap();
    store
        .resolve(&review("f1", Verdict::Deferred, Reason::Unspecified))
        .unwrap();

    let history = store.history("f1").unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].feature_snapshot["facts"]["callers"], 2);
    assert_eq!(history[1].feature_snapshot["facts"]["callers"], 50);
    assert!(history[0].id < history[1].id);
}

#[test]
fn resolve_validates_reasons_and_finding() {
    let mut store = open_with(&[observed("f1", "design/a", "a.go")]);
    for (verdict, reason) in [
        (Verdict::Rejected, Reason::Unspecified),
        (Verdict::Rejected, Reason::Fixed),
        (Verdict::Confirmed, Reason::FalsePositive),
        (Verdict::Deferred, Reason::AcceptedDebt),
    ] {
        let error = store.resolve(&review("f1", verdict, reason)).unwrap_err();
        assert!(matches!(error, Error::Reason(_)), "{verdict} {reason}");
    }
    assert!(store.history("f1").unwrap().is_empty());
    let unknown = store.resolve(&review("zz", Verdict::Confirmed, Reason::Fixed));
    assert!(matches!(unknown, Err(Error::UnknownFinding(_))));
}

#[test]
fn review_log_is_append_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = Store::path_in(dir.path());
    let mut store = Store::open(&path).unwrap();
    store
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    store
        .resolve(&review("f1", Verdict::Confirmed, Reason::Fixed))
        .unwrap();

    let raw = Connection::open(&path).unwrap();
    let update = raw.execute("UPDATE review_events SET verdict = 'rejected'", []);
    assert!(update.unwrap_err().to_string().contains("append-only"));
    let delete = raw.execute("DELETE FROM review_events", []);
    assert!(delete.unwrap_err().to_string().contains("append-only"));
    assert_eq!(store.history("f1").unwrap().len(), 1);
}

#[test]
fn suppressed_follows_the_latest_verdict() {
    let mut store = open_with(&[
        observed("rejected", "design/a", "a.go"),
        observed("confirmed", "design/a", "b.go"),
        observed("reconsidered", "design/a", "c.go"),
        observed("broad", "design/a", "d.go"),
        observed("unreviewed", "design/a", "e.go"),
    ]);
    store
        .resolve(&review(
            "rejected",
            Verdict::Rejected,
            Reason::FalsePositive,
        ))
        .unwrap();
    store
        .resolve(&review(
            "confirmed",
            Verdict::Confirmed,
            Reason::AcceptedDebt,
        ))
        .unwrap();
    store
        .resolve(&review(
            "reconsidered",
            Verdict::Rejected,
            Reason::ProjectAllowed,
        ))
        .unwrap();
    store
        .resolve(&review("broad", Verdict::Rejected, Reason::ScopeTooBroad))
        .unwrap();
    assert_eq!(
        store.suppressed().unwrap(),
        ["broad", "reconsidered", "rejected"]
            .map(str::to_owned)
            .into()
    );

    store
        .resolve(&review(
            "reconsidered",
            Verdict::Deferred,
            Reason::Unspecified,
        ))
        .unwrap();
    let suppressed = store.suppressed().unwrap();
    assert!(!suppressed.contains("reconsidered"));
    assert_eq!(suppressed.len(), 2);
}

#[test]
fn list_filters_by_rule_and_status() {
    let mut store = open_with(&[
        observed("a1", "design/a", "a.go"),
        observed("a2", "design/a", "b.go"),
        observed("b1", "design/b", "c.go"),
    ]);
    store
        .resolve(&review("a2", Verdict::Rejected, Reason::NotWorthFixing))
        .unwrap();
    store
        .record(&Run {
            observed: vec![
                observed("a2", "design/a", "b.go"),
                observed("b1", "design/b", "c.go"),
            ],
            ..run(vec![])
        })
        .unwrap();

    let fingerprints = |filter: &Filter| -> Vec<String> {
        store
            .list(filter)
            .unwrap()
            .into_iter()
            .map(|f| f.fingerprint)
            .collect()
    };
    assert_eq!(fingerprints(&all()), ["a1", "a2", "b1"]);
    let open = Filter {
        rule: None,
        status: StatusFilter::Open,
    };
    assert_eq!(fingerprints(&open), ["b1"]);
    let suppressed = Filter {
        rule: None,
        status: StatusFilter::Suppressed,
    };
    assert_eq!(fingerprints(&suppressed), ["a2"]);
    let rule = Filter {
        rule: Some("design/b".to_owned()),
        status: StatusFilter::All,
    };
    assert_eq!(fingerprints(&rule), ["b1"]);
    let listed = store.list(&suppressed).unwrap();
    let latest = listed[0].review.unwrap();
    assert_eq!(latest.verdict, Verdict::Rejected);
    assert_eq!(latest.reason, Reason::NotWorthFixing);
    assert!(listed[0].suppressed);
}

#[test]
fn observed_records_a_diagnostic() {
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
    diagnostic.evidence = json!({ "n": 1 });
    let record = Observed::from_diagnostic(&diagnostic, json!({ "language": "go" }));
    assert_eq!(record.fingerprint, diagnostic.fingerprint.as_str());
    assert_eq!(record.path, "src/a.go");
    assert_eq!(record.locator["span"]["start"]["line"], 3);
    assert_eq!(record.symbol.as_deref(), Some("m::f#function"));
    assert_eq!(record.severity, Severity::Warn);
}

#[test]
fn suppressions_flag_the_findings_that_ask_to_narrow_a_rule() {
    let dir = tempfile::tempdir().unwrap();
    let path = Store::path_in(dir.path());
    let mut store = Store::open(&path).unwrap();
    store
        .record(&run(vec![
            observed("broad", "design/a", "a.go"),
            observed("wrong", "design/a", "b.go"),
        ]))
        .unwrap();
    store
        .resolve(&review("broad", Verdict::Rejected, Reason::ScopeTooBroad))
        .unwrap();
    store
        .resolve(&review("wrong", Verdict::Rejected, Reason::FalsePositive))
        .unwrap();

    let raw = Connection::open(&path).unwrap();
    let narrowing = |fingerprint: &str| -> bool {
        raw.query_row(
            "SELECT narrowing FROM suppressions WHERE fingerprint = ?1",
            [fingerprint],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert!(narrowing("broad"));
    assert!(!narrowing("wrong"));
}
