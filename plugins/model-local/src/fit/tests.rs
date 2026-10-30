use super::*;

/// `n` rows per class on two clusters of the unit circle, one group each,
/// the clusters `gap` radians apart when `gap` is 1.0 (separable) and on
/// top of each other when it is 0.0.
fn rows(n: usize, gap: f64) -> Vec<Line> {
    let mut rows = Vec::new();
    for label in [1.0, 0.0] {
        for i in 0..n {
            let angle = i as f64 * 0.01 + if label > 0.5 { 0.0 } else { gap * 1.5 };
            rows.push(Line {
                vector: vec![angle.cos(), angle.sin()],
                label,
                weight: 1.0,
                group: format!("{label}-{i}"),
                ..Line::default()
            });
        }
    }
    rows
}

#[test]
fn fewer_than_ten_labels_in_a_class_train_the_rate_and_say_why() {
    let mut data = rows(20, 1.0);
    data.retain(|r| r.label < 0.5 || r.group.ends_with("-1") || r.group.ends_with("-2"));
    let fitted = fit(&data, 0.9);

    assert_eq!(fitted.chosen, "rate");
    assert_eq!(fitted.candidates.len(), 3);
    for skipped in &fitted.candidates[1..] {
        assert_eq!(
            skipped.skipped.as_deref(),
            Some("fewer than 10 labels per class")
        );
    }
    assert!(matches!(fitted.model, Model::Rate(_)));
    assert_eq!(fitted.auroc, None);
}

#[test]
fn a_separable_set_with_enough_labels_chooses_a_learner_that_beats_the_rate() {
    let fitted = fit(&rows(15, 1.0), 0.9);
    let rate = fitted.candidates[0].brier.unwrap();

    assert_ne!(fitted.chosen, "rate");
    assert!(fitted.brier < rate / 2.0, "{fitted:?}");
    assert!(fitted.auroc.unwrap() > 0.99);
}

#[test]
fn the_simplest_candidate_within_one_standard_error_wins() {
    let scored = |at, brier, se| Scored {
        at,
        brier,
        se,
        p: Vec::new(),
    };
    let all = [
        scored(0, 0.26, 0.01),
        scored(1, 0.20, 0.06),
        scored(2, 0.19, 0.06),
    ];
    // 0.26 is not within 0.19 + 0.06; 0.20 is.
    assert_eq!(one_standard_error(&all).at, 1);
    let close = [scored(0, 0.21, 0.01), scored(1, 0.20, 0.06)];
    assert_eq!(one_standard_error(&close).at, 0);
}

#[test]
fn the_probability_of_a_row_out_of_fold_never_reads_its_own_label() {
    let data = rows(12, 1.0);
    let mut flipped = data.clone();
    flipped[3].label = 1.0 - flipped[3].label;
    let logistic = CANDIDATES[2].1;
    let at = |data: &[Line]| {
        let refs: Vec<&Line> = data.iter().collect();
        out_of_fold(&refs, |train, test| {
            let model = logistic(train);
            test.iter().map(|row| model.predict(row)).collect()
        })[3]
    };

    assert_eq!(at(&data), at(&flipped));
}

#[test]
fn training_twice_gives_the_same_model() {
    let data = rows(12, 1.0);
    assert_eq!(fit(&data, 0.9), fit(&data, 0.9));
}
