//! Model selection: every candidate is scored by its out-of-fold Brier score
//! (calibration included, fitted inside each fold), and the simplest whose
//! score is within one standard error of the best is refitted on all rows.

use serde::Serialize;

use crate::{
    band::{Band, auroc, band},
    candidates::{CANDIDATES, MIN_PER_CLASS, Model},
    folds::{brier, out_of_fold, squared_errors},
    rows::Line,
};

/// What was fitted, and how it was chosen.
#[derive(Debug, Clone, PartialEq)]
pub struct Fitted {
    pub model: Model,
    pub chosen: &'static str,
    /// Every candidate, in the order of the table.
    pub candidates: Vec<Candidate>,
    /// The chosen candidate's out-of-fold Brier score and AUROC.
    pub brier: f64,
    pub auroc: Option<f64>,
    pub band: Band,
}

/// A candidate as the card tells it: scored, or skipped with the reason.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brier: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub se: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// A candidate that was scored.
struct Scored {
    at: usize,
    brier: f64,
    se: f64,
    p: Vec<f64>,
}

/// Fits the candidates on `rows` and keeps the one the data supports.
/// `precision` is what the card's band asks of a fail and of a pass.
pub fn fit(rows: &[Line], precision: f64) -> Fitted {
    let refs: Vec<&Line> = rows.iter().collect();
    let mut candidates = Vec::new();
    let mut scored: Vec<Scored> = Vec::new();
    for (at, (name, fitter)) in CANDIDATES.iter().enumerate() {
        if let Some(reason) = skip(name, &refs) {
            candidates.push(Candidate {
                name,
                brier: None,
                se: None,
                skipped: Some(reason),
            });
            continue;
        }
        let p = out_of_fold(&refs, |train, test| {
            let model = fitter(train);
            test.iter().map(|row| model.predict(row)).collect()
        });
        let (brier, se) = score(&p, &refs);
        candidates.push(Candidate {
            name,
            brier: Some(brier),
            se: Some(se),
            skipped: None,
        });
        scored.push(Scored { at, brier, se, p });
    }
    let chosen = one_standard_error(&scored);
    let (name, fitter) = CANDIDATES[chosen.at];
    Fitted {
        model: fitter(&refs),
        chosen: name,
        candidates,
        brier: chosen.brier,
        // A constant says nothing about ranking.
        auroc: (name != "rate").then(|| auroc(&chosen.p, &refs)).flatten(),
        band: band(&chosen.p, &refs, precision),
    }
}

/// Why a candidate cannot be fitted on the rows; the rate always can.
fn skip(name: &str, rows: &[&Line]) -> Option<String> {
    if name == "rate" {
        return None;
    }
    let fails = rows.iter().filter(|r| r.label > 0.5).count();
    if fails < MIN_PER_CLASS || rows.len() - fails < MIN_PER_CLASS {
        return Some(format!("fewer than {MIN_PER_CLASS} labels per class"));
    }
    let groups: std::collections::BTreeSet<&str> = rows.iter().map(|r| r.group.as_str()).collect();
    (groups.len() < 2).then(|| "fewer than 2 groups".to_owned())
}

/// The weighted Brier score of `p`, and its standard error over the rows.
fn score(p: &[f64], rows: &[&Line]) -> (f64, f64) {
    let mean = brier(p, rows);
    let total: f64 = rows.iter().map(|r| r.weight).sum();
    let variance: f64 = squared_errors(p, rows)
        .iter()
        .zip(rows)
        .map(|(e, r)| r.weight * (e - mean).powi(2))
        .sum::<f64>()
        / total;
    (mean, (variance / rows.len() as f64).sqrt())
}

/// The first, so the simplest, whose Brier score is within one standard
/// error of the best. There is always one candidate scored: the rate.
fn one_standard_error(scored: &[Scored]) -> &Scored {
    let best = scored
        .iter()
        .min_by(|a, b| a.brier.total_cmp(&b.brier))
        .unwrap_or(&scored[0]);
    scored
        .iter()
        .find(|j| j.brier <= best.brier + best.se)
        .unwrap_or(best)
}

#[cfg(test)]
mod tests;
