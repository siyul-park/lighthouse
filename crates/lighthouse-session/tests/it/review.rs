//! Judgments: who judges, which findings wait, and recording a judgment.

use std::path::Path;

use lighthouse_session::{
    AgentKind, Attribution, CheckRequest, Judgment, Session,
    review::{self, NewJudgment, Query, StatusFilter},
};

use crate::fixture::{LONG, project, repo, session, trusted, write};

fn remember(session: &Session) -> Vec<String> {
    let checked = lighthouse_session::check(
        session,
        &CheckRequest {
            paths: vec![session.root.clone()],
            store: true,
            ..CheckRequest::default()
        },
    )
    .unwrap();
    checked
        .findings()
        .iter()
        .map(|d| d.fingerprint.as_str().to_owned())
        .collect()
}

fn kim() -> Attribution {
    Attribution::new(AgentKind::Person, Some("kim".to_owned()))
}

fn judgment(fingerprint: &str, judgment: Judgment) -> NewJudgment {
    NewJudgment {
        fingerprint: fingerprint.to_owned(),
        judgment,
        reason: Some("checked by hand".to_owned()),
        suppress: None,
        attribution: kim(),
        commit: None,
        expect_seen: None,
        lighthouse_version: "test".to_owned(),
    }
}

fn waiting(root: &Path) -> usize {
    let query = Query {
        rules: Vec::new(),
        status: StatusFilter::Open,
        all_tiers: false,
    };
    review::list(root, &query)
        .unwrap()
        .map_or(0, |listed| listed.findings.len())
}

#[test]
fn attribute_to() {
    let default = Attribution::new(AgentKind::SoftwareAgent, Some("hook".to_owned()));
    assert_eq!(review::attribute_to(None, &default).unwrap(), default);

    let person = review::attribute_to(Some("person:kim"), &default).unwrap();
    assert_eq!(
        (person.kind, person.id.as_deref()),
        (AgentKind::Person, Some("kim"))
    );

    // A missing id is the default's when the kind is the same.
    let agent = review::attribute_to(Some("agent"), &default).unwrap();
    assert_eq!(
        (agent.kind, agent.id.as_deref()),
        (AgentKind::SoftwareAgent, Some("hook"))
    );
    let anonymous = review::attribute_to(Some("person"), &default).unwrap();
    assert_eq!(anonymous.id, None);

    let refused = review::attribute_to(Some("robot:r2"), &default)
        .err()
        .unwrap();
    assert!(
        refused.to_string().contains("is not person or agent"),
        "{refused}"
    );
}

#[test]
fn person() {
    let dir = repo("warn");
    let named = review::person(dir.path());
    assert_eq!(
        (named.kind, named.id.as_deref()),
        (AgentKind::Person, Some("Kim"))
    );
}

#[test]
fn list() {
    let dir = project("warn");
    // Nothing is remembered before a check.
    assert!(
        review::list(
            dir.path(),
            &Query {
                rules: Vec::new(),
                status: StatusFilter::Open,
                all_tiers: false
            }
        )
        .unwrap()
        .is_none()
    );

    remember(&session(dir.path()));
    assert_eq!(waiting(dir.path()), 1);

    let elsewhere = review::list(
        dir.path(),
        &Query {
            rules: vec!["core/*".to_owned()],
            status: StatusFilter::Open,
            all_tiers: false,
        },
    )
    .unwrap()
    .unwrap();
    assert!(elsewhere.findings.is_empty());

    let errors = project("error");
    remember(&session(errors.path()));
    assert_eq!(waiting(errors.path()), 0, "an error is no review task");
    let all = review::list(
        errors.path(),
        &Query {
            rules: Vec::new(),
            status: StatusFilter::Open,
            all_tiers: true,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(all.findings.len(), 1);
}

#[test]
fn show() {
    let dir = project("warn");
    let fingerprint = remember(&session(dir.path())).remove(0);

    let found = review::show(dir.path(), &fingerprint[..12]).unwrap();
    assert_eq!(found.fingerprint, fingerprint);
    assert_eq!(found.decision, "local/short");

    let short = review::show(dir.path(), &fingerprint[..3]).err().unwrap();
    assert!(short.to_string().contains("at least"), "{short}");
    assert!(review::show(project("warn").path(), &fingerprint).is_err());
}

#[test]
fn show_with_brief() {
    let dir = project("warn");
    let fingerprint = remember(&session(dir.path())).remove(0);

    // The fix does not run in a project the user has not trusted: the finding
    // comes with the brief for fixing it by hand.
    let untrusted = session(dir.path());
    let shown = review::show_with_brief(dir.path(), Some(&untrusted), &fingerprint).unwrap();
    assert_eq!(shown.finding.fingerprint, fingerprint);
    assert!(
        shown
            .brief
            .as_deref()
            .is_some_and(|b| b.contains("local/short"))
    );
    assert!(shown.fix.is_some_and(|fix| !fix.runs));

    // A fix that is `unsafe` also asks for a brief; without a session there
    // is neither.
    let bare = review::show_with_brief(dir.path(), None, &fingerprint).unwrap();
    assert!(bare.brief.is_none() && bare.fix.is_none());

    let trusted = trusted(dir.path());
    let shown = review::show_with_brief(dir.path(), Some(&trusted), &fingerprint).unwrap();
    assert!(shown.fix.is_some_and(|fix| fix.runs));
}

#[test]
fn history() {
    let dir = project("warn");
    let fingerprint = remember(&session(dir.path())).remove(0);
    assert!(
        review::history(dir.path(), &fingerprint)
            .unwrap()
            .is_empty()
    );

    review::resolve(dir.path(), &judgment(&fingerprint, Judgment::Pass)).unwrap();
    review::resolve(dir.path(), &judgment(&fingerprint, Judgment::Fail)).unwrap();
    let events = review::history(dir.path(), &fingerprint[..10]).unwrap();
    let judged: Vec<Judgment> = events.iter().map(|e| e.spec.judgment).collect();
    assert_eq!(judged, [Judgment::Pass, Judgment::Fail]);
}

#[test]
fn prune() {
    let dir = project("warn");
    remember(&session(dir.path()));
    // Open findings stay.
    assert_eq!(review::prune(dir.path(), None).unwrap(), 0);

    // Once the code is fixed, the finding is resolved and nobody judged it.
    write(dir.path(), "a.txt", "a\n");
    assert!(remember(&session(dir.path())).is_empty());
    assert_eq!(review::prune(dir.path(), Some(30)).unwrap(), 0, "too young");
    assert_eq!(review::prune(dir.path(), None).unwrap(), 1);
    assert!(review::prune(project("warn").path(), None).is_err());
}

#[test]
fn resolve() {
    let dir = project("warn");
    let fingerprint = remember(&session(dir.path())).remove(0);

    let recorded = review::resolve(dir.path(), &judgment(&fingerprint, Judgment::Pass)).unwrap();
    assert_eq!(recorded.event.spec.judgment, Judgment::Pass);
    // The judgment is stamped with the versions of the decision it is about.
    assert_eq!(
        recorded.event.spec.decision_uid.as_deref(),
        Some("3f2a9c1e-5b7d-4e8f-9a1b-2c3d4e5f6a7b")
    );
    assert!(recorded.standing.is_some_and(|s| s.stands()));
    assert!(recorded.warnings.is_empty());
    assert_eq!(waiting(dir.path()), 0, "a judged warning waits for no one");

    let short = review::resolve(dir.path(), &judgment("abc", Judgment::Pass));
    assert!(short.is_err());

    // A finding that is gone says so to whoever judges it late.
    let gone = project("warn");
    let fingerprint = remember(&session(gone.path())).remove(0);
    write(gone.path(), "a.txt", "a\n");
    remember(&session(gone.path()));
    let late = review::resolve(gone.path(), &judgment(&fingerprint, Judgment::Fail)).unwrap();
    assert!(
        late.warnings.iter().any(|w| w.contains("already resolved")),
        "{:?}",
        late.warnings
    );
}

#[test]
fn resolve_all() {
    let dir = project("warn");
    write(dir.path(), "b.txt", LONG);
    let found = remember(&session(dir.path()));
    assert_eq!(found.len(), 2);

    let reviews = [
        judgment(&found[0], Judgment::Pass),
        judgment("0000000000000000", Judgment::Pass),
        judgment(&found[1], Judgment::Fail),
    ];
    let done = review::resolve_all(dir.path(), &reviews).unwrap();
    assert_eq!(done.len(), 3);
    assert!(done[0].is_ok());
    assert!(
        done[1].is_err(),
        "each judgment succeeds or fails by itself"
    );
    assert!(done[2].is_ok());
    assert_eq!(waiting(dir.path()), 0, "both judged findings stand");
}
