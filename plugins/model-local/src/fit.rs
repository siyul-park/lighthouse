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
mod tests {
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
}
