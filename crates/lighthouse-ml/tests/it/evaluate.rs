use std::{fs, path::PathBuf};

use lighthouse_engine::Outcome as Run;
use lighthouse_ml::{
    evaluate::{Candidate, EvalRequest, Found, Gathered, Repo, Setup, grade, repos},
    labels::{Label, Shown, Source},
    metrics::{Outcome, Truth},
};
use lighthouse_model::{Diagnostic, Fingerprint, Position, Severity, Span};
use lighthouse_spec::{Catalog, Evaluation};
use serde_json::json;

const RULE: &str = "core/file-size";

fn request(audit: Option<usize>) -> EvalRequest {
    EvalRequest {
        decision: RULE.to_owned(),
        repos: Vec::new(),
        baseline: None,
        candidate: None,
        config: None,
        audit,
    }
}

fn repo() -> Repo {
    Repo {
        name: Some("cel".to_owned()),
        dir: PathBuf::from("."),
    }
}

fn label(key: &str, truth: Truth, observed: Option<Outcome>) -> Label {
    Label {
        key: key.to_owned(),
        truth,
        weight: 1.0,
        source: Source::Person,
        attribution: json!({}),
        observed,
        subject: None,
    }
}

fn flagged(fingerprint: &str, file: &str) -> Diagnostic {
    Diagnostic::new(
        RULE,
        Severity::Warn,
        "m",
        file,
        Span {
            start: Position { line: 1, col: 1 },
            end: Position { line: 1, col: 2 },
        },
        Fingerprint::from_raw(fingerprint),
    )
}

fn found(flags: &[(&str, &str)]) -> Found {
    Found::of(&Run {
        diagnostics: flags.iter().map(|(f, file)| flagged(f, file)).collect(),
        ..Run::default()
    })
}

fn policy() -> Evaluation {
    Evaluation {
        precision: 0.5,
        recall: None,
    }
}

#[test]
fn repos_defaults_to_the_start_directory_and_joins_named_ones_to_it() {
    let start = PathBuf::from("/work");
    let own = repos(&[], &start);
    assert_eq!(own.len(), 1);
    assert_eq!(
        (own[0].name.as_deref(), own[0].dir.clone()),
        (None, start.clone())
    );

    let named = repos(&[repo()], &start);
    assert_eq!(named[0].name.as_deref(), Some("cel"));
    assert_eq!(named[0].dir, start.join("."));
}

#[test]
fn found_of_a_run_keeps_what_the_check_flagged_by_fingerprint_and_place() {
    let found = found(&[("one", "a.rs")]);
    // A label the run flagged is a `fail` for the check; another is unselected.
    let part = grade(
        &request(None),
        &repo(),
        policy(),
        &[
            label("one", Truth::Fail, None),
            label("two", Truth::Fail, None),
        ],
        &found,
        None,
    );
    let mut gathered = Gathered::default();
    gathered.add(part);
    let report = gathered.finish(&request(None), false).unwrap();
    let recall = report.current.metrics.recall.unwrap();
    assert_eq!((recall.hits, recall.of), (1, 2));
}

#[test]
fn grade_pairs_the_candidate_with_the_current_check_and_reads_examples_as_shown() {
    let current = found(&[("one", "a.rs")]);
    let beside = found(&[]);
    let candidate = Candidate {
        id: "local/next".to_owned(),
        found: &beside,
        shown: vec![Shown {
            key: "example:x".to_owned(),
            truth: Truth::Fail,
            outcome: Outcome::Fail,
            subject: None,
        }],
    };
    let labels = [
        label("one", Truth::Fail, None),
        label("example:x", Truth::Fail, Some(Outcome::Pass)),
    ];
    let part = grade(
        &request(None),
        &repo(),
        policy(),
        &labels,
        &current,
        Some(candidate),
    );
    let mut gathered = Gathered::default();
    gathered.add(part);
    let report = gathered.finish(&request(None), true).unwrap();
    let side = report.candidate.unwrap();
    assert_eq!(side.check, "local/next");
    // The example is flagged by the candidate; the finding it no longer flags.
    let recall = side.metrics.recall.unwrap();
    assert_eq!((recall.hits, recall.of), (1, 2));
    assert_eq!(report.agreement.unwrap().of, 2);
}

#[test]
fn gathered_add_and_finish_pool_the_repositories_into_one_report() {
    let mut gathered = Gathered::default();
    for fingerprint in ["one", "two"] {
        let found = found(&[(fingerprint, "a.rs")]);
        let labels = [label(fingerprint, Truth::Fail, None)];
        gathered.add(grade(
            &request(None),
            &repo(),
            policy(),
            &labels,
            &found,
            None,
        ));
    }
    let report = gathered.finish(&request(None), false).unwrap();
    assert_eq!(report.labelled, 2);
    assert_eq!(report.meets_policy, Some(true));
    assert!(Gathered::default().finish(&request(None), false).is_err());
}

#[test]
fn setup_read_adds_the_candidate_file_to_the_decisions_of_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let text = Catalog::decision_text(Catalog::bundled().decision(RULE).unwrap()).unwrap();
    let file = dir.path().join("candidate.yaml");
    fs::write(
        &file,
        text.replace(RULE, "local/probe")
            .replace("lighthouse/pack: core", "lighthouse/pack: local")
            .replace("lighthouse/section: limits", "lighthouse/section: rules"),
    )
    .unwrap();

    let (id, setup) = Setup::read(dir.path(), &file).unwrap();
    assert_eq!(id, "local/probe");
    assert!(setup.catalog().unwrap().decision("local/probe").is_some());

    fs::write(
        &file,
        "apiVersion: lighthouse/v1alpha1\nkind: Project\nmetadata:\n  name: x/p\n",
    )
    .unwrap();
    assert!(Setup::read(dir.path(), &file).is_err());
}
