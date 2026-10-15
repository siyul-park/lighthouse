//! Weighted L2 logistic regression by Newton's method (iteratively
//! reweighted least squares), and Platt scaling, which is the same fit on one
//! feature. There are few rows and many features, so each step is solved in
//! the space of rows: the kernel `X Xᵀ` is computed once, and a step costs
//! `n³/3` however long the vectors are.

use serde::{Deserialize, Serialize};

/// The most Newton steps of a fit.
const ITERATIONS: usize = 50;
/// A fit has converged when no score moves more than this.
const TOLERANCE: f64 = 1e-8;
/// The smallest `p (1 - p)` a step divides by.
const FLOOR: f64 = 1e-9;
/// How little Platt scaling is regularized: it only keeps a separable set
/// finite.
const PLATT_C: f64 = 100.0;

/// A fitted logistic regression: the score of `x` is its logit.
#[derive(Debug, Clone, PartialEq)]
pub struct Logit {
    pub weights: Vec<f64>,
    pub bias: f64,
}

impl Logit {
    pub fn score(&self, x: &[f64]) -> f64 {
        self.bias + dot(&self.weights, x)
    }
}

/// `p = σ(a·s + b)` for a raw score `s`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Platt {
    pub a: f64,
    pub b: f64,
}

impl Platt {
    /// The scale that changes nothing: a raw score is already a logit.
    pub const IDENTITY: Self = Self { a: 1.0, b: 0.0 };

    pub fn apply(&self, score: f64) -> f64 {
        sigmoid(self.a * score + self.b)
    }
}

/// One Newton step as a weighted ridge regression on the working response.
struct Step {
    /// The weight of each row, `wᵢ pᵢ (1 - pᵢ)`.
    weight: Vec<f64>,
    /// The weighted share of each row, which centres the features.
    centre: Vec<f64>,
    /// The working response `zᵢ`, and its weighted mean.
    response: Vec<f64>,
    mean: f64,
}

impl Step {
    fn of(scores: &[f64], y: &[f64], w: &[f64]) -> Self {
        let mut weight = Vec::with_capacity(scores.len());
        let mut response = Vec::with_capacity(scores.len());
        for ((&f, &label), &row) in scores.iter().zip(y).zip(w) {
            let p = sigmoid(f);
            let q = (p * (1.0 - p)).max(FLOOR);
            weight.push(row * q);
            response.push(f + (label - p) / q);
        }
        let total: f64 = weight.iter().sum();
        let centre: Vec<f64> = weight.iter().map(|v| v / total).collect();
        let mean = centre.iter().zip(&response).map(|(c, z)| c * z).sum();
        Self {
            weight,
            centre,
            response,
            mean,
        }
    }

    /// The coefficients `cᵢ` with `β = Σ cᵢ (xᵢ - x̄)`: the ridge solution
    /// with penalty `lambda` on the centred kernel.
    fn solve(&self, kernel: &[f64], lambda: f64) -> Vec<f64> {
        let n = self.weight.len();
        let means: Vec<f64> = (0..n)
            .map(|i| (0..n).map(|j| self.centre[j] * kernel[i * n + j]).sum())
            .collect();
        let grand: f64 = self.centre.iter().zip(&means).map(|(c, m)| c * m).sum();
        let root: Vec<f64> = self.weight.iter().map(|v| v.sqrt()).collect();
        let mut system = vec![0.0; n * n];
        for i in 0..n {
            for j in 0..n {
                let centred = kernel[i * n + j] - means[i] - means[j] + grand;
                system[i * n + j] = root[i] * centred * root[j] + if i == j { lambda } else { 0.0 };
            }
        }
        let target: Vec<f64> = (0..n)
            .map(|i| root[i] * (self.response[i] - self.mean))
            .collect();
        let solved = solve(system, target);
        solved.iter().zip(&root).map(|(a, r)| a * r).collect()
    }

    /// The scores the coefficients give every row, from the kernel alone.
    fn scores(&self, kernel: &[f64], coefficients: &[f64]) -> Vec<f64> {
        let n = coefficients.len();
        let total: f64 = coefficients.iter().sum();
        let means: Vec<f64> = (0..n)
            .map(|i| (0..n).map(|j| self.centre[j] * kernel[i * n + j]).sum())
            .collect();
        let grand: f64 = self.centre.iter().zip(&means).map(|(c, m)| c * m).sum();
        let shift: f64 = coefficients.iter().zip(&means).map(|(c, m)| c * m).sum();
        (0..n)
            .map(|i| {
                let along: f64 = (0..n).map(|j| coefficients[j] * kernel[i * n + j]).sum();
                self.mean - (shift - total * grand) + along - total * means[i]
            })
            .collect()
    }
}

pub fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// The logit of a probability kept off 0 and 1.
pub fn logit(p: f64) -> f64 {
    let p = p.clamp(1e-3, 1.0 - 1e-3);
    (p / (1.0 - p)).ln()
}

pub fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Fits Platt scaling to raw `scores` of rows with labels `y` (0 or 1) and
/// weights `w`.
pub fn platt(scores: &[f64], y: &[f64], w: &[f64]) -> Platt {
    let x: Vec<Vec<f64>> = scores.iter().map(|&s| vec![s]).collect();
    let fit = fit(&x, y, w, PLATT_C);
    Platt {
        a: fit.weights[0],
        b: fit.bias,
    }
}

/// Minimizes `½|β|² + C Σ wᵢ·loss(yᵢ, b + xᵢ·β)` over `β` and the unpenalized
/// intercept `b`; `y` is 0 or 1.
pub fn fit(x: &[Vec<f64>], y: &[f64], w: &[f64], c: f64) -> Logit {
    let n = x.len();
    let kernel = kernel(x);
    let mut scores = vec![0.0; n];
    let mut coefficients = vec![0.0; n];
    let mut centre = vec![1.0 / n as f64; n];
    for _ in 0..ITERATIONS {
        let step = Step::of(&scores, y, w);
        coefficients = step.solve(&kernel, 1.0 / c);
        centre = step.centre.clone();
        let next = step.scores(&kernel, &coefficients);
        let moved = scores
            .iter()
            .zip(&next)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        scores = next;
        if moved < TOLERANCE {
            break;
        }
    }
    primal(x, &coefficients, &centre, &scores)
}

/// `X Xᵀ` as a row-major matrix.
fn kernel(x: &[Vec<f64>]) -> Vec<f64> {
    let n = x.len();
    let mut kernel = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let value = dot(&x[i], &x[j]);
            kernel[i * n + j] = value;
            kernel[j * n + i] = value;
        }
    }
    kernel
}

/// The weights and intercept the coefficients stand for.
fn primal(x: &[Vec<f64>], coefficients: &[f64], centre: &[f64], scores: &[f64]) -> Logit {
    let total: f64 = coefficients.iter().sum();
    let width = x.first().map_or(0, Vec::len);
    let mut weights = vec![0.0; width];
    for (row, &c) in x.iter().zip(coefficients) {
        for (weight, value) in weights.iter_mut().zip(row) {
            *weight += c * value;
        }
    }
    let mut mean = vec![0.0; width];
    for (row, &share) in x.iter().zip(centre) {
        for (m, value) in mean.iter_mut().zip(row) {
            *m += share * value;
        }
    }
    for (weight, m) in weights.iter_mut().zip(&mean) {
        *weight -= total * m;
    }
    // The first row's score fixes the intercept the weights leave.
    let bias = scores
        .first()
        .zip(x.first())
        .map_or(0.0, |(score, row)| score - dot(&weights, row));
    Logit { weights, bias }
}

/// Solves `A x = b` for a symmetric positive definite `A` (row-major) by
/// Cholesky decomposition.
fn solve(mut a: Vec<f64>, b: Vec<f64>) -> Vec<f64> {
    let n = b.len();
    for j in 0..n {
        let pivot = (a[j * n + j] - (0..j).map(|k| a[j * n + k].powi(2)).sum::<f64>()).max(1e-12);
        a[j * n + j] = pivot.sqrt();
        for i in j + 1..n {
            let sum: f64 = (0..j).map(|k| a[i * n + k] * a[j * n + k]).sum();
            a[i * n + j] = (a[i * n + j] - sum) / a[j * n + j];
        }
    }
    let mut y = vec![0.0; n];
    for i in 0..n {
        let sum: f64 = (0..i).map(|k| a[i * n + k] * y[k]).sum();
        y[i] = (b[i] - sum) / a[i * n + i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let sum: f64 = (i + 1..n).map(|k| a[k * n + i] * x[k]).sum();
        x[i] = (y[i] - sum) / a[i * n + i];
    }
    x
}
