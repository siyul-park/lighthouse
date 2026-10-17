use lighthouse_ml::metrics::{Metrics, Outcome, Rate};
use lighthouse_model::Judgment;

#[test]
fn wilson_matches_the_published_example() {
    // 81 of 263: the textbook interval is about 0.2553 to 0.3662.
    let rate = Rate::new(81, 263).unwrap();
    assert!((rate.low - 0.2553).abs() < 0.001, "{rate:?}");
    assert!((rate.high - 0.3662).abs() < 0.001, "{rate:?}");
    assert!(Rate::new(0, 0).is_none());
    assert_eq!(Rate::new(0, 10).unwrap().low, 0.0);
}

#[test]
fn metrics_follow_a_hand_computed_set() {
    use Judgment::{Fail, NotApplicable};
    use Outcome::Review;
    let pairs = [
        (Fail, Outcome::Fail),
        (Fail, Outcome::Fail),
        (Fail, Outcome::Pass),
        (Fail, Outcome::NotApplicable),
        (Judgment::Pass, Outcome::Fail),
        (Judgment::Pass, Outcome::Pass),
        (Judgment::Pass, Review),
        (NotApplicable, Outcome::Fail),
    ];
    let m = Metrics::of(&pairs, &[]);
    // tp 2 and the notApplicable fail is not scored; fp 1, tn 1, fn 2;
    // selected 7, notApplicable 1, review 1.
    let close = |rate: Option<Rate>, want: f64| {
        assert!((rate.unwrap().value - want).abs() < 1e-9, "{rate:?}");
    };
    close(m.precision, 2.0 / 3.0);
    close(m.recall, 0.5);
    close(m.fpr, 0.5);
    close(m.fnr, 0.5);
    close(m.selector_precision, 6.0 / 7.0);
    close(m.coverage, 6.0 / 7.0);
    assert_eq!(m.labelled, 8);
    assert_eq!((m.answered, m.brier, m.auroc, m.ece), (0, None, None, None));
}

#[test]
fn brier_and_auroc_follow_a_hand_computed_set() {
    // Brier: (0.1^2 + 0.6^2 + 0.4^2 + 0.1^2) / 4. AUROC: the fail pairs win
    // 1, 1, a tie (half) and 1, of 4.
    let scored = [(true, 0.9), (true, 0.4), (false, 0.4), (false, 0.1)];
    let m = Metrics::of(&[], &scored);
    assert_eq!(m.answered, 4);
    assert!((m.brier.unwrap() - 0.135).abs() < 1e-9, "{m:?}");
    assert!((m.auroc.unwrap() - 0.875).abs() < 1e-9, "{m:?}");
    // Bins: [0.1] fail 0 vs 0.1; [0.4, 0.4] one fail vs 0.8; [0.9] fail 1 vs 0.9.
    // (0.1 + 0.2 + 0.1) / 4
    assert!((m.ece.unwrap() - 0.1).abs() < 1e-9, "{m:?}");
}

#[test]
fn auroc_is_none_when_one_class_is_missing_and_brier_when_nothing_was_scored() {
    let only_fails = Metrics::of(&[], &[(true, 0.8), (true, 0.6)]);
    assert!(only_fails.auroc.is_none());
    assert!((only_fails.brier.unwrap() - 0.1).abs() < 1e-9);
    let none = Metrics::of(&[], &[]);
    assert!(none.brier.is_none() && none.auroc.is_none() && none.ece.is_none());
}

#[test]
fn metrics_shortfall_says_how_precision_or_recall_miss_the_policy() {
    use lighthouse_spec::Evaluation;
    let pairs = [
        (Judgment::Fail, Outcome::Fail),
        (Judgment::Pass, Outcome::Fail),
        (Judgment::Fail, Outcome::Pass),
    ];
    let m = Metrics::of(&pairs, &[]);
    let policy = |precision, recall| Evaluation { precision, recall };
    // precision 1/2, recall 1/2
    assert_eq!(
        m.shortfall(policy(0.9, None)).unwrap(),
        "precision 0.50 < 0.90 (n=2)"
    );
    assert_eq!(m.shortfall(policy(0.5, None)), None);
    assert_eq!(
        m.shortfall(policy(0.5, Some(0.9))).unwrap(),
        "recall 0.50 < 0.90 (n=2)"
    );
    assert_eq!(
        Metrics::of(&[], &[]).shortfall(policy(0.9, None)),
        None,
        "no precision to judge"
    );
}
