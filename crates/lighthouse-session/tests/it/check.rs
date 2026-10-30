//! A run of the project's checks and what a frontend reads out of it.

use lighthouse_report::{Detail, Format};
use lighthouse_session::{CheckReport, CheckRequest, Checked, RunStatus, Session};
use serde_json::{Value, json};

use crate::fixture::{LONG, git, project, repo, session, write};

/// The run over the whole project, whatever the working directory.
fn run(session: &Session, mut request: CheckRequest) -> Checked {
    if request.paths.is_empty() {
        request.paths = vec![session.root.clone()];
    }
    lighthouse_session::check(session, &request).unwrap()
}

fn everything(session: &Session) -> Checked {
    run(session, CheckRequest::default())
}

#[test]
fn check() {
    let dir = project("error");
    let checked = everything(&session(dir.path()));
    let findings = checked.findings();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id, "local/short");
    assert_eq!(findings[0].file.to_str(), Some("a.txt"));

    // Only the decisions asked for run; one that is not active is no decision.
    let local = run(
        &session(dir.path()),
        CheckRequest {
            rules: vec!["local/*".to_owned()],
            ..CheckRequest::default()
        },
    );
    assert_eq!(local.findings().len(), 1);
    let unknown = lighthouse_session::check(
        &session(dir.path()),
        &CheckRequest {
            rules: vec!["core/*".to_owned()],
            paths: vec![dir.path().to_owned()],
            ..CheckRequest::default()
        },
    );
    assert!(
        unknown
            .err()
            .unwrap()
            .to_string()
            .contains("no decision matches")
    );

    // A run that stores remembers its findings for review.
    let remembered = run(
        &session(dir.path()),
        CheckRequest {
            store: true,
            ..CheckRequest::default()
        },
    );
    assert_eq!(remembered.findings().len(), 1);
    assert!(
        lighthouse_session::review::list(
            dir.path(),
            &lighthouse_session::review::Query {
                rules: Vec::new(),
                status: lighthouse_session::review::StatusFilter::Open,
                all_tiers: true,
            }
        )
        .unwrap()
        .is_some_and(|listed| listed.findings.len() == 1)
    );
}

#[test]
fn check_reports_only_the_changed_files_of_a_working_tree() {
    let dir = repo("error");
    // `a.txt` is committed with its finding; `b.txt` is new.
    write(dir.path(), "b.txt", LONG);
    let checked = run(
        &session(dir.path()),
        CheckRequest {
            changed: true,
            ..CheckRequest::default()
        },
    );
    let files: Vec<_> = checked
        .findings()
        .iter()
        .map(|d| d.file.display().to_string())
        .collect();
    assert_eq!(files, ["b.txt"]);
    assert!(
        checked
            .messages
            .iter()
            .any(|m| m.contains("reporting 1 changed file(s)")),
        "{:?}",
        checked.messages
    );

    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "second"]);
    git(dir.path(), &["checkout", "-q", "-b", "feature"]);
    write(dir.path(), "c.txt", LONG);
    let since = run(
        &session(dir.path()),
        CheckRequest {
            diff: Some("main".to_owned()),
            ..CheckRequest::default()
        },
    );
    let files: Vec<_> = since
        .findings()
        .iter()
        .map(|d| d.file.display().to_string())
        .collect();
    assert_eq!(files, ["c.txt"]);
}

#[test]
fn checked_summary() {
    let dir = project("error");
    let summary = everything(&session(dir.path())).summary();
    assert_eq!(summary.status, RunStatus::Findings);
    assert_eq!((summary.errors, summary.warnings, summary.infos), (1, 0, 0));
    assert_eq!(summary.reviews, 0, "an error asks for no review");

    write(dir.path(), "a.txt", "a\n");
    let clean = everything(&session(dir.path())).summary();
    assert_eq!(clean.status, RunStatus::Clean);
    assert_eq!(clean.errors, 0);

    let warned = project("warn");
    let summary = everything(&session(warned.path())).summary();
    assert_eq!((summary.errors, summary.warnings), (0, 1));
    assert_eq!(summary.reviews, 1, "a warning waits for review");
}

#[test]
fn checked_findings() {
    let dir = project("error");
    write(dir.path(), "b.txt", LONG);
    let findings = everything(&session(dir.path())).findings();
    let mut files: Vec<_> = findings
        .iter()
        .map(|d| d.file.display().to_string())
        .collect();
    files.sort();
    assert_eq!(files, ["a.txt", "b.txt"]);
}

#[test]
fn checked_render() {
    let dir = project("error");
    let checked = everything(&session(dir.path()));

    let text = checked.render(Format::Text, None);
    assert!(text.contains("a.txt"), "{text}");
    assert!(text.contains("a.txt is too long"), "{text}");

    let agent = checked.render(Format::Agent, Some(1));
    assert!(agent.contains("a.txt"), "{agent}");

    let sarif: Value = serde_json::from_str(&checked.render(Format::Sarif, None)).unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    assert_eq!(
        sarif["runs"][0]["results"][0]["ruleId"], "local/short",
        "{sarif}"
    );

    // The limit cuts the findings the agent format shows.
    write(dir.path(), "b.txt", LONG);
    let two = everything(&session(dir.path()));
    let shown = |limit| {
        two.render(Format::Agent, Some(limit))
            .matches("is too long")
            .count()
    };
    assert!(shown(1) < shown(5));
}

#[test]
fn checked_sarif_with_fixes() {
    let dir = project("error");
    let checked = everything(&session(dir.path()));
    let properties = json!({ "agent": { "runs": 2 } });
    let log = checked.sarif_with_fixes(
        None,
        &std::collections::BTreeMap::new(),
        properties.as_object().unwrap(),
    );
    let log: Value = serde_json::from_str(&log).unwrap();
    assert_eq!(log["runs"][0]["results"].as_array().unwrap().len(), 1);
    assert_eq!(log["runs"][0]["properties"]["agent"]["runs"], 2, "{log}");
}

#[test]
fn checked_report() {
    let dir = project("error");
    let checked = everything(&session(dir.path()));

    let CheckReport::Full {
        status, summary, ..
    } = checked.report(Detail::Full, None)
    else {
        panic!("full detail gives the full report");
    };
    assert_eq!(status, RunStatus::Findings);
    assert_eq!(summary.errors, 1);

    let compact = serde_json::to_value(checked.report(Detail::Compact, None)).unwrap();
    assert!(compact.to_string().contains("local/short"), "{compact}");
    assert!(matches!(
        checked.report(Detail::Compact, None),
        CheckReport::Compact { .. }
    ));
}

#[test]
fn checked_sources() {
    let dir = project("error");
    write(dir.path(), "a.txt", "é\nb\nc\n");
    write(dir.path(), "b.txt", LONG);
    let sources = everything(&session(dir.path())).sources();
    // Only a file with findings and characters outside ASCII is read back.
    assert_eq!(sources.keys().collect::<Vec<_>>(), ["a.txt"]);
    assert_eq!(sources["a.txt"], "é\nb\nc\n");
}

#[test]
fn checked_briefing() {
    let dir = project("warn");
    let checked = everything(&session(dir.path()));
    let briefing = checked.briefing(Some(3));
    assert_eq!(briefing.limit, Some(3));
    assert!(
        briefing
            .catalog
            .is_some_and(|c| c.decision("local/short").is_some())
    );
    let findings = checked.findings();
    assert!(briefing.needs_review(&findings[0]));
}

#[test]
fn remembered() {
    let dir = project("warn");
    let first = run(
        &session(dir.path()),
        CheckRequest {
            store: true,
            ..CheckRequest::default()
        },
    );
    assert_eq!(first.remembered.suppressed, 0);
    let fingerprint = first.findings()[0].fingerprint.as_str().to_owned();

    // A pass judgment keeps the warning out of the next report.
    lighthouse_session::review::resolve(
        dir.path(),
        &lighthouse_session::review::NewJudgment {
            fingerprint,
            judgment: lighthouse_session::Judgment::Pass,
            reason: Some("short notes are optional here".to_owned()),
            suppress: None,
            attribution: lighthouse_session::Attribution::new(
                lighthouse_session::AgentKind::Person,
                Some("kim".to_owned()),
            ),
            commit: None,
            expect_seen: None,
            lighthouse_version: "test".to_owned(),
        },
    )
    .unwrap();
    let second = run(
        &session(dir.path()),
        CheckRequest {
            store: true,
            ..CheckRequest::default()
        },
    );
    assert!(second.findings().is_empty());
    assert_eq!(second.remembered.suppressed, 1);
    assert!(
        second
            .remembered
            .messages
            .iter()
            .any(|m| m.contains("kept out of the report by judgments")),
        "{:?}",
        second.remembered.messages
    );
}
