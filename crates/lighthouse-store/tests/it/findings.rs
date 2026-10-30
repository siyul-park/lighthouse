//! Findings: recorded, resolved, listed and pruned.

use std::thread;

use crate::support::*;
use lighthouse_model::{Diagnostic, Fingerprint, Judgment, Label, Position, Severity, Span};
use lighthouse_store::{
    Error, Filter, Observed, Run, RunSummary, Standing, State, StatusFilter, Store, Unchecked,
};
use serde_json::json;

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
            deactivated: 0
        }
    );
    let first = store.finding("f1").unwrap();
    assert_eq!(first.message, "message of f1");
    assert_eq!(first.first_seen, first.last_seen);
    assert_eq!(first.commit.as_deref(), Some("abc123"));
    assert_eq!(first.dirty, Some(true));
    assert_eq!(first.options["max"], 3);
    assert_eq!(first.authored_severity, Severity::Info);
    assert_eq!(first.lighthouse_version.as_deref(), Some("0.1.0"));
    assert_eq!(first.catalog_version.as_deref(), Some("cat1"));
    assert_eq!(first.state, State::Open);

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
    assert_eq!(resolved.state, State::Resolved);

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
        ran: vec!["design/a".to_owned()],
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
        active: vec!["design/a".to_owned()],
        ran: vec!["design/a".to_owned()],
        ..run(vec![observed("kept", "design/a", "a.go")])
    };
    assert_eq!(store.record(&only_a).unwrap().deactivated, 1);
    let dropped = store.finding("dropped").unwrap();
    assert!(dropped.inactive_at.is_some());
    assert_eq!(dropped.state, State::Inactive);
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
        observed("abcdefg1", "design/a", "a.go"),
        observed("abcdefg2", "design/a", "b.go"),
        observed("xyzwvut0", "design/a", "c.go"),
    ]);
    assert_eq!(store.finding("abcdefg1").unwrap().fingerprint, "abcdefg1");
    assert_eq!(store.finding("xyzwvut").unwrap().fingerprint, "xyzwvut0");
    let Err(Error::AmbiguousFinding { candidates, .. }) = store.finding("abcdefg") else {
        panic!("abcdefg is ambiguous");
    };
    assert_eq!(candidates, ["abcdefg1", "abcdefg2"]);
    let message = store.finding("abcdefg").unwrap_err().to_string();
    assert!(
        message.contains("longer prefix") && message.contains("abcdefg2"),
        "{message}"
    );
    assert!(matches!(
        store.finding("xyz"),
        Err(Error::ShortFingerprint(_))
    ));
    assert!(matches!(
        store.finding("nopenop"),
        Err(Error::UnknownFinding(_))
    ));
}

#[test]
fn resolve_freezes_the_finding_as_it_was_last_seen() {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    let mut input = review("f1", Judgment::Fail);
    input.reason = Some("hot path".to_owned());
    input.suppress = Some("named policy".to_owned());
    let resolved = store.resolve(&input, stamp("sem1")).unwrap();
    thread::sleep(std::time::Duration::from_millis(2));
    let event = &resolved.event;
    assert_eq!(event.spec.fingerprint, "f1");
    assert_eq!(event.spec.decision, "design/a");
    assert_eq!(event.spec.judgment, Judgment::Fail);
    assert_eq!(event.spec.reason.as_deref(), Some("hot path"));
    assert_eq!(event.suppressions.len(), 1);
    assert_eq!(event.suppressions[0].spec.justification, "named policy");
    assert_eq!(event.spec.meaning_version.as_deref(), Some("sem1"));
    assert_eq!(event.spec.decision_hash.as_deref(), Some("full1"));
    assert_eq!(event.spec.catalog_version.as_deref(), Some("cat1"));
    assert_eq!(event.spec.lighthouse_version.as_deref(), Some("0.1.0"));
    assert_eq!(event.spec.language.as_deref(), Some("go"));
    assert_eq!(event.spec.scope.as_deref(), Some("symbol"));
    assert_eq!(event.spec.commit.as_deref(), Some("def456"));
    assert_eq!(event.spec.was_attributed_to.id.as_deref(), Some("claude"));
    assert_eq!(event.label(), Label::Separate);
    assert_eq!(event.id.len(), 32);
    assert_eq!(resolved.finding.fingerprint, "f1");

    let snapshot = &event.spec.snapshot;
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

    let mut changed = observed("f1", "design/a", "a.go");
    changed.facts = json!({ "language": "go", "callers": 50 });
    store.record(&run(vec![changed])).unwrap();
    judge(&mut store, "f1", Judgment::Pass);
    let history = store.history("f1").unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].spec.snapshot["facts"]["callers"], 2);
    assert_eq!(history[1].spec.snapshot["facts"]["callers"], 50);
}

#[test]
fn resolve_validates_suppressions_findings_and_the_sighting() {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    for judgment in [Judgment::Pass, Judgment::NotApplicable] {
        let mut input = review("f1", judgment);
        input.suppress = Some("why".to_owned());
        let error = store.resolve(&input, stamp("sem1")).unwrap_err();
        assert!(
            matches!(error, Error::SuppressionWithoutFail(j) if j == judgment),
            "{judgment}"
        );
    }
    assert!(store.history("f1").unwrap().is_empty());
    let unknown = store.resolve(&review("zzzzzzz", Judgment::Fail), stamp("sem1"));
    assert!(matches!(unknown, Err(Error::UnknownFinding(_))));

    let seen = store.finding("f1").unwrap().last_seen;
    let mut stale = review("f1", Judgment::Fail);
    stale.expect_seen = Some("2000-01-01T00:00:00.000Z".to_owned());
    assert!(matches!(
        store.resolve(&stale, stamp("sem1")),
        Err(Error::Changed { .. })
    ));
    let mut current = review("f1", Judgment::Fail);
    current.expect_seen = Some(seen);
    store.resolve(&current, stamp("sem1")).unwrap();
}

#[test]
fn list_filters_by_rule_and_status() {
    let mut store = memory_with(&[
        observed("a1", "design/a", "a.go"),
        observed("a2", "design/a", "b.go"),
        observed("b1", "design/b", "c.go"),
        observed("gone", "design/b", "d.go"),
    ]);
    judge_suppressed(&mut store, "a2", "won't-fix");
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
        decision: Some("design/b".to_owned()),
        status: StatusFilter::All,
    };
    assert_eq!(fingerprints(&store, &rule), ["b1", "gone"]);
    let listed = store.list(&status(StatusFilter::Suppressed)).unwrap();
    assert_eq!(listed[0].judgment, Some(Judgment::Fail));
    assert_eq!(listed[0].justification.as_deref(), Some("won't-fix"));
    assert_eq!(listed[0].standing, Some(Standing::Suppressed));
}

#[test]
fn prune_removes_only_unjudged_findings_that_are_resolved_or_inactive() {
    let mut store = memory_with(&[
        observed("open", "design/a", "a.go"),
        observed("resolved", "design/a", "b.go"),
        observed("reviewed", "design/a", "c.go"),
    ]);
    judge(&mut store, "reviewed", Judgment::Fail);
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
        Fingerprint::new("design/a", "m::f#function", ""),
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

#[test]
fn a_finding_starts_where_its_locator_says_and_at_zero_without_a_span() {
    let mut located = observed("f1", "design/a", "a.go");
    located.locator = json!({ "span": { "start": { "line": 7, "col": 3 } } });
    let mut bare = observed("f2", "design/a", "a.go");
    bare.locator = json!({});
    let store = memory_with(&[located, bare]);
    let at = store.finding("f1").unwrap();
    assert_eq!((at.line(), at.col()), (7, 3));
    let nowhere = store.finding("f2").unwrap();
    assert_eq!((nowhere.line(), nowhere.col()), (0, 0));
}
