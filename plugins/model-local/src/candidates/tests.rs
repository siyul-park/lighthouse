use super::*;

/// Two clusters on the unit circle: fails near (1, 0), passes near (0, 1).
fn cluster(label: f64, n: usize) -> Vec<Line> {
    (0..n)
        .map(|i| {
            let angle = (i as f64 - n as f64 / 2.0) * 0.01;
            let base = if label > 0.5 {
                0.0
            } else {
                std::f64::consts::FRAC_PI_2
            };
            Line {
                vector: vec![(base + angle).cos(), (base + angle).sin()],
                label,
                weight: 1.0,
                group: format!("g{}-{i}", label),
                ..Line::default()
            }
        })
        .collect()
}

fn separable() -> Vec<Line> {
    let mut rows = cluster(1.0, 12);
    rows.extend(cluster(0.0, 12));
    rows
}

fn near(vector: [f64; 2]) -> Line {
    Line {
        vector: vector.to_vec(),
        weight: 1.0,
        group: "q".to_owned(),
        ..Line::default()
    }
}

#[test]
fn the_rate_is_the_weighted_share_of_fails_shrunk_by_half_a_row() {
    let rows = [
        Line {
            weight: 2.0,
            ..cluster(1.0, 1).remove(0)
        },
        Line {
            weight: 1.0,
            ..cluster(0.0, 1).remove(0)
        },
    ];
    let refs: Vec<&Line> = rows.iter().collect();
    // (2 + 0.5) / (3 + 1)
    assert_eq!(prior(&refs), 0.625);
}

#[test]
fn a_vote_is_the_cosine_weighted_share_of_the_nearest_shrunk_toward_the_rate() {
    let model = Knn {
        k: 2,
        rate: 0.5,
        labels: vec![1.0, 0.0, 1.0],
        weights: vec![1.0, 0.5, 1.0],
        rows: vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![-1.0, 0.0]],
        platt: Platt::IDENTITY,
    };
    // Asked at (1, 0) the nearest are the first (sim 1) and the second
    // (sim 0, which does not vote): the share is 1, and
    // (1·2 + 0.5·2) / (2 + 2).
    assert_eq!(vote(&model, &[1.0, 0.0]), 0.75);
    // Where every neighbour is opposed no one votes: the rate remains.
    let opposed = Knn {
        rows: vec![vec![-1.0, 0.0]; 3],
        ..model
    };
    assert_eq!(vote(&opposed, &[1.0, 0.0]), 0.5);
}

#[test]
fn logistic_regression_separates_a_separable_set_and_beats_the_rate() {
    let rows = separable();
    let refs: Vec<&Line> = rows.iter().collect();
    let logistic = fit_logistic(&refs);
    let rate = fit_rate(&refs);

    let p = |model: &Model, vector: [f64; 2]| model.predict(&near(vector));
    assert!(p(&logistic, [1.0, 0.0]) > 0.9);
    assert!(p(&logistic, [0.0, 1.0]) < 0.1);
    let loss = |model: &Model| {
        let predicted: Vec<f64> = refs.iter().map(|r| model.predict(r)).collect();
        brier(&predicted, &refs)
    };
    assert!(loss(&logistic) < loss(&rate) / 4.0);
}

#[test]
fn knn_separates_the_same_set_after_calibration() {
    let rows = separable();
    let refs: Vec<&Line> = rows.iter().collect();
    let model = fit_knn(&refs);

    assert!(model.predict(&near([1.0, 0.0])) > 0.8);
    assert!(model.predict(&near([0.0, 1.0])) < 0.2);
}

#[test]
fn similarities_are_the_max_and_the_mean_per_side() {
    let sims = similarities(&[1.0, 0.0], &[vec![1.0, 0.0], vec![0.0, 1.0]], &[]);
    assert_eq!(sims, [1.0, 0.5, 0.0, 0.0]);
}
