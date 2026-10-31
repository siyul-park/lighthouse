//! What the hooks record of a session, and what comes of it: signals in the
//! local store, the decisions proposed from them, and what the agent is told.

use std::{thread, time::Duration};

use lighthouse_model::SignalClass;
use lighthouse_session::{
    AgentKind, Attribution, CheckRequest, Session,
    signal::{self, Signal},
};
use serde_json::{Value, json};

use crate::fixture::{canonical, project, project_with, session};

const DIRECTIVE: &str = "From now on, never use unwrap() in library code.";
const SECOND: &str = "Always return errors with context from now on.";

fn kim() -> Attribution {
    Attribution::new(AgentKind::Person, Some("kim".to_owned()))
}

fn hook() -> Attribution {
    Attribution::new(AgentKind::SoftwareAgent, Some("hook".to_owned()))
}

fn said(session: &Session, prompt: &str) -> Signal {
    signal::capture_prompt(session, "s1", prompt).unwrap()
}

fn counted(session: &Session, all: bool) -> signal::SignalReport {
    signal::report(session, all, None).unwrap()
}

fn findings(session: &Session) -> Vec<lighthouse_model::Diagnostic> {
    lighthouse_session::check(
        session,
        &CheckRequest {
            paths: vec![session.root.clone()],
            ..CheckRequest::default()
        },
    )
    .unwrap()
    .findings()
}

/// A decision of the project, written as a proposal that derives from `signal`.
fn proposal(name: &str, requirement: &str, signal: &str) -> Value {
    json!({
        "kind": "Decision",
        "metadata": { "name": format!("local/{name}") },
        "spec": {
            "title": "Library code does not unwrap",
            "context": "A panic in a library surprises its callers.",
            "scope": { "subject": "file" },
            "requirement": requirement,
            "severity": "warn",
            "provenance": { "wasDerivedFrom": [format!("signal:{signal}")] },
            "check": {
                "type": "cel", "select": "file", "where": "file.path.endsWith(\".unwrap\")",
                "message": "{{ file.path }} unwraps"
            },
            "examples": [
                { "name": "bad", "language": "text", "kind": "invalid",
                  "files": [{ "path": "x.unwrap", "body": "x" }], "expect": [{ "line": 1 }] },
                { "name": "good", "language": "text", "kind": "valid",
                  "files": [{ "path": "x.txt", "body": "x" }] }
            ]
        }
    })
}

#[test]
fn session_surfaces() {
    let quiet = project("error");
    assert!(!session(quiet.path()).surfaces(), "shadow is the default");
    let shadow = project_with("error", "[signals]\nsurface = \"shadow\"\n");
    assert!(!session(shadow.path()).surfaces());
    let loud = project_with("error", "[signals]\nsurface = \"on\"\n");
    assert!(session(loud.path()).surfaces());
}

#[test]
fn change() {
    let dir = project("error");
    let session = session(dir.path());
    let found = findings(&session);
    signal::capture_findings(
        &session,
        "s1",
        "Stop",
        &found,
        &["0123456789abcdef".to_owned()],
        &hook(),
    )
    .unwrap();
    // What an edit did to a finding is the first word of its signal.
    let mut starts: Vec<String> = counted(&session, true)
        .signals
        .iter()
        .map(|s| s.excerpt.split(' ').next().unwrap().to_owned())
        .collect();
    starts.sort();
    assert_eq!(starts, ["introduced", "resolved"]);
}

#[test]
fn a_signal_older_than_the_retention_is_purged_by_the_next_one() {
    let dir = project_with("error", "[signals]\nretention = \"1s\"\n");
    let session = session(dir.path());
    said(&session, DIRECTIVE);
    thread::sleep(Duration::from_millis(2100));
    said(&session, SECOND);
    assert_eq!(counted(&session, true).counts.total, 1);
}

#[test]
fn capture_prompt() {
    let dir = project("error");
    let session = session(dir.path());

    // Every prompt is recorded, whatever it says.
    let directive = said(&session, DIRECTIVE);
    assert_eq!(directive.class, SignalClass::Directive);
    assert_eq!(directive.by.kind, AgentKind::Person);
    assert!(!directive.rule.is_empty());
    assert_eq!(directive.session, "s1");
    let ack = said(&session, "thanks");
    assert_eq!(ack.class, SignalClass::None);
    let plain = said(&session, "Add a --verbose flag to the check command.");
    assert_eq!(plain.class, SignalClass::Abstain);
    assert_eq!(plain.excerpt, "Add a --verbose flag to the check command.");
    assert_eq!(counted(&session, true).counts.total, 3);

    // The text kept is scrubbed of secrets.
    let key = format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8");
    let leaked = said(&session, &format!("Never commit {key} to the repository."));
    assert!(!leaked.excerpt.contains("ghp_"), "{}", leaked.excerpt);
}

#[test]
fn capture_findings() {
    let dir = project("error");
    let session = session(dir.path());
    let found = findings(&session);
    assert_eq!(found.len(), 1);
    let resolved = ["0123456789abcdef".to_owned()];

    // Nothing introduced and nothing resolved records nothing.
    signal::capture_findings(&session, "s1", "PostToolUse", &[], &[], &hook()).unwrap();
    assert_eq!(counted(&session, true).counts.total, 0);

    signal::capture_findings(&session, "s1", "PostToolUse", &found, &resolved, &hook()).unwrap();
    // A finding seen again by the next hook is recorded once.
    signal::capture_findings(&session, "s1", "Stop", &found, &resolved, &hook()).unwrap();
    let all = counted(&session, true);
    assert_eq!(all.counts.by_class["finding"], 2);
    assert!(
        all.signals
            .iter()
            .all(|s| s.by.id.as_deref() == Some("hook"))
    );
    assert!(
        all.signals
            .iter()
            .any(|s| s.excerpt.contains("local/short") && s.rule == "local/short")
    );
}

#[test]
fn unasked() {
    let dir = project("error");
    let root = canonical(dir.path());
    assert!(
        signal::unasked(&root, "s1").unwrap().is_empty(),
        "no store yet"
    );

    let session = session(dir.path());
    said(&session, DIRECTIVE);
    said(&session, SECOND);
    said(&session, "thanks");
    let open = signal::unasked(&root, "s1").unwrap();
    assert_eq!(open.len(), 2, "only what states a decision");
    assert!(signal::unasked(&root, "other").unwrap().is_empty());
}

#[test]
fn mark_asked() {
    let dir = project("error");
    let root = canonical(dir.path());
    let session = session(dir.path());
    said(&session, DIRECTIVE);
    said(&session, SECOND);

    let open = signal::unasked(&root, "s1").unwrap();
    signal::mark_asked(&root, &open[..1]).unwrap();
    let left = signal::unasked(&root, "s1").unwrap();
    assert_eq!(left.len(), 1);
    assert_ne!(left[0].id, open[0].id);
    signal::mark_asked(&root, &left).unwrap();
    assert!(signal::unasked(&root, "s1").unwrap().is_empty());
    // The signals themselves stay.
    assert_eq!(counted(&session, true).counts.total, 2);
}

#[test]
fn report() {
    let dir = project("error");
    let session = session(dir.path());
    let empty = signal::report(&session, false, None).unwrap();
    assert_eq!(empty.counts.total, 0);

    said(&session, DIRECTIVE);
    said(&session, SECOND);
    said(&session, "thanks");
    said(&session, "Add a --verbose flag to the check command.");

    let open = signal::report(&session, false, None).unwrap();
    assert_eq!(open.signals.len(), 2, "open signals that state a decision");
    assert_eq!(open.counts.total, 4);
    assert_eq!(open.counts.abstained, 1);
    assert_eq!(open.counts.by_class["directive"], 2);
    assert_eq!(open.counts.by_class["none"], 1);
    assert_eq!(open.counts.by_status["open"], 4);

    let all = signal::report(&session, true, Some(3)).unwrap();
    assert_eq!((all.signals.len(), all.omitted), (3, 1));

    // Nothing recorded yet from the signals: nothing is captured or proposed.
    let capture = open.measures.capture.unwrap();
    assert_eq!((capture.hits, capture.of), (0, 2));
    assert!(open.measures.duplicate.is_none());
}

#[test]
fn dismiss() {
    let dir = project("error");
    let session = session(dir.path());
    let first = said(&session, DIRECTIVE);

    let refused = signal::dismiss(&session, &first.id, "  ", &kim());
    assert!(
        refused
            .err()
            .unwrap()
            .to_string()
            .contains("needs a reason")
    );
    let missing = signal::dismiss(&session, "ffffffffffff", "not a rule", &kim());
    assert!(missing.is_err());

    let done = signal::dismiss(&session, &first.id[..8], "a request, not a rule", &kim()).unwrap();
    assert_eq!(done.status.name(), "dismissed");
    assert_eq!(done.status.detail(), Some("a request, not a rule"));
    assert_eq!(done.settled_by.as_deref(), Some("person:kim"));
    assert!(
        signal::report(&session, false, None)
            .unwrap()
            .signals
            .is_empty()
    );
    assert_eq!(counted(&session, true).counts.by_status["dismissed"], 1);
}

#[test]
fn candidates() {
    let dir = project("error");
    let session = session(dir.path());
    let query = "A note MUST NOT run past two lines. Notes are short.";

    let found = signal::candidates(&session, query, None, 0.5, 3).unwrap();
    assert_eq!(found[0].id, "local/short");
    assert!(found[0].similarity >= 0.5 && found[0].similarity <= 1.0);
    assert_eq!(found[0].requirement, "A note MUST NOT run past two lines.");

    let skipped = signal::candidates(&session, query, Some("local/short"), 0.5, 3).unwrap();
    assert!(skipped.iter().all(|d| d.id != "local/short"));
    let nothing = signal::candidates(&session, "quantum entanglement", None, 0.5, 3).unwrap();
    assert!(nothing.is_empty());
    assert!(
        signal::candidates(&session, query, None, 0.5, 0)
            .unwrap()
            .len()
            <= 1
    );
}

#[test]
fn support() {
    let dir = project("error");
    let first = said(&session(dir.path()), DIRECTIVE);
    let agent_said = {
        let session = session(dir.path());
        signal::capture_findings(&session, "s1", "Stop", &findings(&session), &[], &hook())
            .unwrap();
        counted(&session, true)
            .signals
            .into_iter()
            .find(|s| s.class == SignalClass::Finding)
            .unwrap()
    };

    let mut document = proposal("no-unwrap", "Library code MUST NOT unwrap.", &first.id);
    document["spec"]["provenance"]["wasDerivedFrom"]
        .as_array_mut()
        .unwrap()
        .extend([
            json!(format!("signal:{}", agent_said.id)),
            json!("signal:ffffffffffff"),
        ]);
    let made = lighthouse_session::create_decision(&session(dir.path()), document, &kim()).unwrap();
    assert_eq!(made.signals.len(), 2);
    assert_eq!(made.unknown_signals, ["ffffffffffff"]);

    let session = session(dir.path());
    let decision = session.catalog.decision("local/no-unwrap").unwrap();
    let support = signal::support(&session, decision).unwrap().unwrap();
    assert_eq!(support.signals, 2);
    assert_eq!(support.by_class["directive"]["person"], 1);
    assert_eq!(support.by_class["finding"]["agent"], 1);
    assert_eq!(support.person_share, 0.5, "(1 + 1) / (2 + 2)");

    // A decision that names no signal has no support to count.
    let other = session.catalog.decision("local/short").unwrap();
    assert!(signal::support(&session, other).unwrap().is_none());
}

#[test]
fn support_line() {
    let dir = project("error");
    let first = said(&session(dir.path()), DIRECTIVE);
    lighthouse_session::create_decision(
        &session(dir.path()),
        proposal("no-unwrap", "Library code MUST NOT unwrap.", &first.id),
        &kim(),
    )
    .unwrap();
    let session = session(dir.path());
    let decision = session.catalog.decision("local/no-unwrap").unwrap();
    let line = signal::support(&session, decision).unwrap().unwrap().line();
    assert_eq!(line, "1 signal(s) (directive: person 1), person share 0.67");
}

#[test]
fn read() {
    let reading =
        signal::read_prompt("Please look.\n```\nnever panic\n```\nFrom now on, use tabs.");
    assert_eq!(
        reading.stripped.text,
        "Please look.\nFrom now on, use tabs."
    );
    assert!(reading.stripped.had_code);
    let hit = reading.hit.unwrap();
    assert_eq!(hit.class, SignalClass::Directive);
    assert_eq!(hit.sentence, "From now on, use tabs.");
    assert!(signal::read_prompt("```\nnever panic\n```").hit.is_none());
}

#[test]
fn classify() {
    let hit = signal::classify("Never use unwrap() in library code.").unwrap();
    assert_eq!(hit.class, SignalClass::Directive);
    assert_eq!(hit.sentence, "Never use unwrap() in library code.");
    // A question is not a decision, and neither is a request with no rule.
    assert!(signal::classify("Should we never use unwrap()?").is_none());
    assert!(signal::classify("Add a --verbose flag.").is_none());
    assert_eq!(signal::classify("thanks").unwrap().class, SignalClass::None);
}

#[test]
fn truncated() {
    assert_eq!(signal::truncated("short"), "short");
    let long = "x".repeat(300);
    let cut = signal::truncated(&long);
    assert_eq!(cut.chars().count(), signal::EXCERPT_CHARS);
    assert!(cut.ends_with('…'));
    let wide = "한".repeat(300);
    assert_eq!(
        signal::truncated(&wide).chars().count(),
        signal::EXCERPT_CHARS
    );
}

#[test]
fn features() {
    let stripped = signal::stripped("에러는 항상 Result로 반환해야 해.\n```\nlet x = 1;\n```");
    let features = signal::features("원문", &stripped);
    assert_eq!(features["language"], "ko");
    assert_eq!(features["chars"], 2);
    assert_eq!(features["hadCode"], true);
    assert_eq!(features["hadPasted"], false);
    let english = signal::features("x", &signal::stripped("Always use tabs."));
    assert_eq!(english["language"], "en");
    assert_eq!(
        signal::features("", &signal::stripped(""))["language"],
        "none"
    );
}

#[test]
fn signal_id() {
    let id = signal::signal_id(&["s1", "prompt", "text"]);
    assert_eq!(id.len(), 12);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(id, signal::signal_id(&["s1", "prompt", "text"]));
    assert_ne!(id, signal::signal_id(&["s2", "prompt", "text"]));
    // Where one part ends matters: the parts are not run together.
    assert_ne!(
        signal::signal_id(&["ab", "c"]),
        signal::signal_id(&["a", "bc"])
    );
}

#[test]
fn prompt_context() {
    let dir = project("error");
    let session = session(dir.path());

    let directive = said(&session, "From now on, a note must not run past two lines.");
    let context = signal::prompt_context(&session, &directive).unwrap();
    assert!(
        context.contains(&format!("signal {}", directive.id)),
        "{context}"
    );
    assert!(context.contains("decision_create"), "{context}");
    assert!(context.contains(&format!("signal:{}", directive.id)));
    assert!(
        context.contains("local/short"),
        "the decision that may already say it: {context}"
    );

    // Only a message that states a decision is told about.
    assert!(signal::prompt_context(&session, &said(&session, "thanks")).is_none());
    let plain = said(&session, "Add a --verbose flag to the check command.");
    assert!(signal::prompt_context(&session, &plain).is_none());

    let unrelated = said(&session, "From now on, quantum entanglement is forbidden.");
    let context = signal::prompt_context(&session, &unrelated).unwrap();
    assert!(!context.contains("Existing decisions"), "{context}");
}

#[test]
fn stop_context() {
    let dir = project("error");
    let session = session(dir.path());
    let first = said(&session, DIRECTIVE);
    let second = said(&session, SECOND);
    let text = signal::stop_context(&[first.clone(), second.clone()]);
    assert!(
        text.starts_with("Lighthouse: 2 signal(s) of this session are open:"),
        "{text}"
    );
    assert!(
        text.contains(&format!("- {} [directive] {DIRECTIVE}", first.id)),
        "{text}"
    );
    assert!(text.contains(&second.id));
    assert!(text.contains("decision_signal_dismiss"));
}

#[test]
fn digest() {
    let dir = project("error");
    let session = session(dir.path());
    let first = said(&session, DIRECTIVE);

    let plain = signal::digest(&session).unwrap().unwrap();
    assert!(
        plain.starts_with("Lighthouse decisions of this project ("),
        "{plain}"
    );
    assert!(!plain.contains("Proposals that wait"), "{plain}");
    assert!(plain.lines().filter(|l| l.starts_with("- ")).count() <= 6);

    // A proposal waits for a person, with the support it has.
    lighthouse_session::create_decision(
        &session,
        proposal("no-unwrap", "Library code MUST NOT unwrap.", &first.id),
        &kim(),
    )
    .unwrap();
    let with = signal::digest(&crate::fixture::session(dir.path()))
        .unwrap()
        .unwrap();
    assert!(
        with.contains("Proposals that wait for a person to accept them:"),
        "{with}"
    );
    assert!(
        with.contains("- local/no-unwrap [1 signal(s) (directive: person 1), person share 0.67]"),
        "{with}"
    );
}
