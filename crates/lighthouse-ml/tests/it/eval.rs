use lighthouse_engine::Run;
use lighthouse_ml::{
    eval::{Candidate, EvalReport, Found, RepoResult, Usage, grade},
    labels::{Label, Source},
    metrics::Outcome,
};
use lighthouse_model::{Diagnostic, Fingerprint, Judgment, Position, Severity, Span};
use lighthouse_spec::Evaluation;
use serde_json::json;

const RULE: &str = "core/max-lines";

fn graded(repo: &str, labels: &[Label], found: &Found, candidate: Option<Candidate>) -> RepoResult {
    grade(RULE, None, Some(repo), policy(), labels, found, candidate)
}

fn label(key: &str, judgment: Judgment, observed: Option<Outcome>) -> Label {
    Label {
        key: key.to_owned(),
        judgment,
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
fn found_of_a_run_keeps_what_the_check_flagged_by_fingerprint_and_place() {
    let found = found(&[("one", "a.rs")]);
    // A label the run flagged is a `fail` for the check; another is unselected.
    let result = graded(
        "cel",
        &[
            label("one", Judgment::Fail, None),
            label("two", Judgment::Fail, None),
        ],
        &found,
        None,
    );
    let report = EvalReport::pool(vec![result]).unwrap();
    let recall = report.current.metrics.recall.unwrap();
    assert_eq!((recall.hits, recall.of), (1, 2));
}

#[test]
fn the_probabilities_models_gave_score_the_labelled_subjects_they_answered() {
    let mut run = Run {
        diagnostics: vec![flagged("one", "a.rs")],
        reviews: vec![flagged("two", "a.rs"), flagged("three", "a.rs")],
        ..Run::default()
    };
    run.probabilities = [("one", 0.95), ("two", 0.2)]
        .into_iter()
        .map(|(fingerprint, p)| (fingerprint.to_owned(), (p, "m/x@1".to_owned())))
        .collect();
    let labels = [
        label("one", Judgment::Fail, None),
        label("two", Judgment::Pass, None),
        label("three", Judgment::Pass, None),
    ];
    let result = graded("cel", &labels, &Found::of(&run), None);
    let metrics = EvalReport::pool(vec![result]).unwrap().current.metrics;
    // "three" abstained: counted in the labels, not scored.
    assert_eq!(metrics.labelled, 3);
    assert_eq!(metrics.answered, 2);
    assert!((metrics.brier.unwrap() - (0.0025 + 0.04) / 2.0).abs() < 1e-9);
    assert_eq!(metrics.auroc, Some(1.0));
}

#[test]
fn pool_keeps_each_score_of_every_repository_sorted_by_repo_and_fingerprint() {
    let mut results = Vec::new();
    for (name, fingerprint, judgment, p) in [
        ("zed", "b", Judgment::Fail, 0.9),
        ("cel", "c", Judgment::Pass, 0.2),
        ("cel", "a", Judgment::NotApplicable, 0.5),
        ("cel", "b", Judgment::Fail, 0.7),
    ] {
        let mut run = Run {
            diagnostics: vec![flagged(fingerprint, "a.rs")],
            ..Run::default()
        };
        run.probabilities = [(fingerprint.to_owned(), (p, "m/x@1".to_owned()))].into();
        results.push(graded(
            name,
            &[label(fingerprint, judgment, None)],
            &Found::of(&run),
            None,
        ));
    }
    let side = EvalReport::pool(results).unwrap().current;
    // The `notApplicable` label is about the selector and is not scored.
    let kept: Vec<_> = side
        .scores
        .iter()
        .map(|s| {
            (
                s.repo.as_deref(),
                s.fingerprint.as_str(),
                s.violates,
                s.probability,
            )
        })
        .collect();
    assert_eq!(
        kept,
        [
            (Some("cel"), "b", true, 0.7),
            (Some("cel"), "c", false, 0.2),
            (Some("zed"), "b", true, 0.9),
        ]
    );
    assert_eq!(side.metrics.answered, 3);
    assert!(side.scores.iter().all(|s| s.model == "m/x@1"));
}

#[test]
fn the_notices_of_the_run_are_kept_on_the_side_without_repeats() {
    let results = (0..2)
        .map(|_| {
            let run = Run {
                notices: ["model m/x failed".to_owned()].into(),
                ..Run::default()
            };
            graded("cel", &[], &Found::of(&run), None)
        })
        .collect();
    let side = EvalReport::pool(results).unwrap().current;

    assert_eq!(side.notices, ["model m/x failed"]);
}

#[test]
fn grade_pairs_the_candidate_with_the_current_check_and_reads_examples_as_shown() {
    let current = found(&[("one", "a.rs")]);
    let beside = found(&[]);
    let shown = [label("example:x", Judgment::Fail, Some(Outcome::Fail))];
    let candidate = Candidate {
        id: "local/next".to_owned(),
        found: &beside,
        labels: &shown,
    };
    let labels = [
        label("one", Judgment::Fail, None),
        label("example:x", Judgment::Fail, Some(Outcome::Pass)),
    ];
    let result = graded("cel", &labels, &current, Some(candidate));
    let report = EvalReport::pool(vec![result]).unwrap();
    let side = report.candidate.unwrap();
    assert_eq!(side.check, "local/next");
    // The example is flagged by the candidate; the finding it no longer flags.
    let recall = side.metrics.recall.unwrap();
    assert_eq!((recall.hits, recall.of), (1, 2));
    assert_eq!(report.agreement.unwrap().of, 2);
}

#[test]
fn pool_puts_the_repositories_into_one_report() {
    let results = ["one", "two"]
        .into_iter()
        .map(|fingerprint| {
            let found = found(&[(fingerprint, "a.rs")]);
            let labels = [label(fingerprint, Judgment::Fail, None)];
            graded("cel", &labels, &found, None)
        })
        .collect();
    let report = EvalReport::pool(results).unwrap();
    assert_eq!(report.labelled, 2);
    assert_eq!(report.meets_policy, Some(true));
    assert!(EvalReport::pool(Vec::new()).is_err());
}

#[test]
fn usage_sums_what_each_repository_asked_of_models() {
    let usages =
        [(1, 2, 3, 0.5), (2, 0, 1, 0.25)].map(|(model_decided, reviews, calls, cost)| Usage {
            model_decided,
            reviews,
            calls,
            cost,
        });
    let sum: Usage = usages.into_iter().sum();
    assert_eq!(
        (sum.model_decided, sum.reviews, sum.calls, sum.cost),
        (3, 2, 4, 0.75)
    );
}
