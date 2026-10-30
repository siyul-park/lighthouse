//! The learners, in the order of their complexity. Each is a `fit_*` on rows
//! and a `predict_*` of P(fail) for one row; the table says which are
//! candidates, and [`Model`] dispatches a fitted one to its `predict_*`.

use serde::{Deserialize, Serialize};

use crate::{
    folds::{brier, out_of_fold},
    newton::{self, Design, Platt, dot, logit, sigmoid},
    rows::Line,
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

pub type Fit = fn(&[&Line]) -> Model;

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
    pub fn predict(&self, row: &Line) -> f64 {
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

fn fit_rate(rows: &[&Line]) -> Model {
    Model::Rate(Rate { rate: prior(rows) })
}

fn fit_knn(rows: &[&Line]) -> Model {
    let raw = out_of_fold(rows, |train, test| {
        let model = neighbours(train, Platt::IDENTITY);
        test.iter()
            .map(|r| logit(vote(&model, &r.vector)))
            .collect()
    });
    let (labels, weights) = labels_of(rows);
    Model::Knn(neighbours(rows, newton::platt(&raw, &labels, &weights)))
}

fn neighbours(rows: &[&Line], platt: Platt) -> Knn {
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
fn prior(rows: &[&Line]) -> f64 {
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

fn predict_knn(model: &Knn, row: &Line) -> f64 {
    model.platt.apply(logit(vote(model, &row.vector)))
}

fn fit_logistic(rows: &[&Line]) -> Model {
    let (labels, weights) = labels_of(rows);
    // Chosen inside the rows given, never on the ones that will judge it: each
    // fold's kernel is built once for every strength.
    let raws = out_of_fold(rows, |train, test| {
        let fits = regressions(train, &C_GRID);
        test.iter()
            .map(|r| {
                let x = features(r);
                fits.iter().map(|fit| fit.score(&x)).collect::<Vec<f64>>()
            })
            .collect()
    });
    let tried = |at: usize| {
        let raw: Vec<f64> = raws.iter().map(|scores| scores[at]).collect();
        let p: Vec<f64> = raw.iter().copied().map(sigmoid).collect();
        (at, brier(&p, rows), raw)
    };
    let (at, _, raw) =
        (1..C_GRID.len()).map(tried).fold(
            tried(0),
            |best, next| if next.1 < best.1 { next } else { best },
        );
    let c = C_GRID[at];
    let fit = regressions(rows, &[c]).remove(0);
    Model::Logistic(Logistic {
        c,
        weights: fit.weights,
        bias: fit.bias,
        platt: newton::platt(&raw, &labels, &weights),
    })
}

/// The regression on the rows at each strength in `cs`.
fn regressions(rows: &[&Line], cs: &[f64]) -> Vec<newton::Logit> {
    let x: Vec<Vec<f64>> = rows.iter().map(|r| features(r)).collect();
    let (labels, weights) = labels_of(rows);
    let design = Design::new(&x, &labels, &weights);
    cs.iter().map(|&c| design.fit(c)).collect()
}

fn predict_logistic(model: &Logistic, row: &Line) -> f64 {
    model
        .platt
        .apply(dot(&model.weights, &features(row)) + model.bias)
}

fn features(row: &Line) -> Vec<f64> {
    row.vector.iter().chain(&row.sims).copied().collect()
}

fn labels_of(rows: &[&Line]) -> (Vec<f64>, Vec<f64>) {
    (
        rows.iter().map(|r| r.label).collect(),
        rows.iter().map(|r| r.weight).collect(),
    )
}

#[cfg(test)]
mod tests;
