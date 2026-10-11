use lighthouse_session::metrics::{Outcome, Rate, Truth, agreement, evaluate};

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
    use Outcome::{Review, Unselected};
    use Truth::{Fail, NotApplicable};
    let pairs = [
        (Fail, Outcome::Fail),
        (Fail, Outcome::Fail),
        (Fail, Outcome::Pass),
        (Fail, Unselected),
        (Truth::Pass, Outcome::Fail),
        (Truth::Pass, Outcome::Pass),
        (Truth::Pass, Review),
        (NotApplicable, Outcome::Fail),
    ];
    let m = evaluate(&pairs);
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
    close(m.abstention, 1.0 / 7.0);
    assert_eq!(m.labelled, 8);
}

#[test]
fn agreement_counts_equal_outcomes() {
    use Outcome::{Fail, Pass, Review};
    let rate = agreement(&[(Fail, Fail), (Pass, Fail), (Review, Review)]).unwrap();
    assert_eq!((rate.hits, rate.of), (2, 3));
    assert!(agreement(&[]).is_none());
}
