//! The learners, in the order of their complexity. Each is a `fit_*` on rows
//! and a `predict_*` of P(fail) for one row; the table says which are
//! candidates, and [`Model`] dispatches a fitted one to its `predict_*`.

use serde::{Deserialize, Serialize};

use crate::{
    folds::{Row, brier, out_of_fold},
    newton::{self, Platt, dot, logit, sigmoid},
};

/// The neighbours that vote.
pub const K: usize = 5;
/// The weight, in neighbours, of the rate a vote is shrunk toward.
const PSEUDO: f64 = 2.0;
/// The regularization strengths logistic regression tries.
const C_GRID: [f64; 4] = [0.1, 0.3, 1.0, 3.0];
/// The labels of each class a learner beyond the rate needs.
pub const MIN_PER_CLASS: usize = 10;

/// The candidates by name, simplest first.
pub const CANDIDATES: [(&str, Fit); 3] = [
    ("rate", fit_rate),
    ("knn", fit_knn),
    ("logistic", fit_logistic),
];

pub type Fit = fn(&[&Row]) -> Model;

/// A fitted learner, as the artifact stores it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Model {
    Rate(Rate),
    Knn(Knn),
    Logistic(Logistic),
}

impl Model {
    /// P(fail) of the row.
    pub fn predict(&self, row: &Row) -> f64 {
        match self {
            Self::Rate(model) => model.rate,
            Self::Knn(model) => predict_knn(model, row),
            Self::Logistic(model) => predict_logistic(model, row),
        }
    }

    /// Whether the model reads the row's vector.
    pub fn embeds(&self) -> bool {
        !matches!(self, Self::Rate(_))
    }
}

/// The weighted share of fails among the rows, whatever the row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rate {
    pub rate: f64,
}

/// The rows nearest to the one asked vote, shrunk toward the rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Knn {
    pub k: usize,
    pub rate: f64,
    pub labels: Vec<f64>,
    pub weights: Vec<f64>,
    /// The vectors of the rows, which the artifact keeps among its references.
    #[serde(skip)]
    pub rows: Vec<Vec<f64>>,
    pub platt: Platt,
}

/// L2 logistic regression on the vector and its similarities to the
/// examples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Logistic {
    pub c: f64,
    pub weights: Vec<f64>,
    pub bias: f64,
    pub platt: Platt,
}

/// Maximum and mean cosine of `vector` to the invalid examples, then to the
/// valid ones; 0 for a side without examples. Vectors are unit length.
pub fn similarities(vector: &[f64], invalid: &[Vec<f64>], valid: &[Vec<f64>]) -> [f64; 4] {
    let side = |examples: &[Vec<f64>]| {
        let sims: Vec<f64> = examples.iter().map(|e| dot(vector, e)).collect();
        match sims.iter().copied().reduce(f64::max) {
            Some(max) => [max, sims.iter().sum::<f64>() / sims.len() as f64],
            None => [0.0, 0.0],
        }
    };
    let (bad, good) = (side(invalid), side(valid));
    [bad[0], bad[1], good[0], good[1]]
}

fn fit_rate(rows: &[&Row]) -> Model {
    Model::Rate(Rate { rate: prior(rows) })
}

fn fit_knn(rows: &[&Row]) -> Model {
    let raw = out_of_fold(rows, |train, test| {
        let model = neighbours(train, Platt::IDENTITY);
        test.iter()
            .map(|r| logit(vote(&model, &r.vector)))
            .collect()
    });
    let (labels, weights) = labels_of(rows);
    Model::Knn(neighbours(rows, newton::platt(&raw, &labels, &weights)))
}

fn neighbours(rows: &[&Row], platt: Platt) -> Knn {
    let (labels, weights) = labels_of(rows);
    Knn {
        k: K,
        rate: prior(rows),
        labels,
        weights,
        rows: rows.iter().map(|r| r.vector.clone()).collect(),
        platt,
    }
}

/// `(Σ w·y + ½) / (Σ w + 1)`.
fn prior(rows: &[&Row]) -> f64 {
    let fails: f64 = rows.iter().map(|r| r.weight * r.label).sum();
    let all: f64 = rows.iter().map(|r| r.weight).sum();
    (fails + 0.5) / (all + 1.0)
}

/// `(vote·k + rate·2) / (k + 2)`, the vote being the cosine-weighted share of
/// fails among the `k` nearest rows.
fn vote(model: &Knn, vector: &[f64]) -> f64 {
    let mut near: Vec<(f64, usize)> = model
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| (dot(vector, row), i))
        .collect();
    near.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let (mut fails, mut all) = (0.0, 0.0);
    for &(sim, i) in near.iter().take(model.k) {
        let share = sim.max(0.0) * model.weights[i];
        fails += share * model.labels[i];
        all += share;
    }
    let share = if all > 0.0 { fails / all } else { model.rate };
    let k = model.k as f64;
    (share * k + model.rate * PSEUDO) / (k + PSEUDO)
}

fn predict_knn(model: &Knn, row: &Row) -> f64 {
    model.platt.apply(logit(vote(model, &row.vector)))
}

fn fit_logistic(rows: &[&Row]) -> Model {
    let (labels, weights) = labels_of(rows);
    // Chosen inside the rows given, never on the ones that will judge it.
    let tried = |c: f64| {
        let raw = out_of_fold(rows, |train, test| {
            let fit = fit_regression(train, c);
            test.iter().map(|r| fit.score(&features(r))).collect()
        });
        let p: Vec<f64> = raw.iter().copied().map(sigmoid).collect();
        (c, brier(&p, rows), raw)
    };
    let [first, rest @ ..] = C_GRID;
    let (c, _, raw) =
        rest.into_iter().map(tried).fold(
            tried(first),
            |best, next| if next.1 < best.1 { next } else { best },
        );
    let fit = fit_regression(rows, c);
    Model::Logistic(Logistic {
        c,
        weights: fit.weights,
        bias: fit.bias,
        platt: newton::platt(&raw, &labels, &weights),
    })
}

fn fit_regression(rows: &[&Row], c: f64) -> newton::Logit {
    let x: Vec<Vec<f64>> = rows.iter().map(|r| features(r)).collect();
    let (labels, weights) = labels_of(rows);
    newton::fit(&x, &labels, &weights, c)
}

fn predict_logistic(model: &Logistic, row: &Row) -> f64 {
    model
        .platt
        .apply(dot(&model.weights, &features(row)) + model.bias)
}

fn features(row: &Row) -> Vec<f64> {
    row.vector.iter().chain(&row.sims).copied().collect()
}

fn labels_of(rows: &[&Row]) -> (Vec<f64>, Vec<f64>) {
    (
        rows.iter().map(|r| r.label).collect(),
        rows.iter().map(|r| r.weight).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two clusters on the unit circle: fails near (1, 0), passes near (0, 1).
    fn cluster(label: f64, n: usize) -> Vec<Row> {
        (0..n)
            .map(|i| {
                let angle = (i as f64 - n as f64 / 2.0) * 0.01;
                let base = if label > 0.5 {
                    0.0
                } else {
                    std::f64::consts::FRAC_PI_2
                };
                Row {
                    vector: vec![(base + angle).cos(), (base + angle).sin()],
                    sims: [0.0; 4],
                    label,
                    weight: 1.0,
                    group: format!("g{}-{i}", label),
                }
            })
            .collect()
    }

    fn separable() -> Vec<Row> {
        let mut rows = cluster(1.0, 12);
        rows.extend(cluster(0.0, 12));
        rows
    }

    fn near(vector: [f64; 2]) -> Row {
        Row {
            vector: vector.to_vec(),
            sims: [0.0; 4],
            label: 0.0,
            weight: 1.0,
            group: "q".to_owned(),
        }
    }

    #[test]
    fn the_rate_is_the_weighted_share_of_fails_shrunk_by_half_a_row() {
        let rows = [
            Row {
                weight: 2.0,
                ..cluster(1.0, 1).remove(0)
            },
            Row {
                weight: 1.0,
                ..cluster(0.0, 1).remove(0)
            },
        ];
        let refs: Vec<&Row> = rows.iter().collect();
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
        let refs: Vec<&Row> = rows.iter().collect();
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
        let refs: Vec<&Row> = rows.iter().collect();
        let model = fit_knn(&refs);

        assert!(model.predict(&near([1.0, 0.0])) > 0.8);
        assert!(model.predict(&near([0.0, 1.0])) < 0.2);
    }

    #[test]
    fn similarities_are_the_max_and_the_mean_per_side() {
        let sims = similarities(&[1.0, 0.0], &[vec![1.0, 0.0], vec![0.0, 1.0]], &[]);
        assert_eq!(sims, [1.0, 0.5, 0.0, 0.0]);
    }
}
