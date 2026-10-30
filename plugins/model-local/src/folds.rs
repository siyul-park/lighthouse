//! How the rows learners fit are split into folds by group (so that
//! subjects of one group, one file, are never in both the part a model is
//! fitted on and the part it is judged on), and how a probability on them is
//! judged.

use lighthouse_protocol::stats::assign;

use crate::rows::Line;

/// The weighted mean of the squared distance of `p`, a probability of each row
/// failing, from its label.
pub fn brier(p: &[f64], rows: &[&Line]) -> f64 {
    let (error, weight) = squared_errors(p, rows)
        .iter()
        .zip(rows)
        .fold((0.0, 0.0), |(e, w), (x, r)| {
            (e + r.weight * x, w + r.weight)
        });
    error / weight
}

/// The squared distance of each probability from its row's label.
pub fn squared_errors(p: &[f64], rows: &[&Line]) -> Vec<f64> {
    p.iter()
        .zip(rows)
        .map(|(p, r)| (p - r.label).powi(2))
        .collect()
}

/// What `score` gives for every row when each is left out with its fold:
/// `score(train, test)` fits on `train` and gives one value per row of `test`.
/// With a single group there is nothing to leave out, and the rows are scored
/// by a fit on themselves.
pub fn out_of_fold<T>(rows: &[&Line], score: impl Fn(&[&Line], &[&Line]) -> Vec<T>) -> Vec<T> {
    let tests = folds(rows);
    if tests.len() < 2 {
        return score(rows, rows);
    }
    let mut scored: Vec<Option<T>> = rows.iter().map(|_| None).collect();
    for test in tests {
        let train: Vec<&Line> = (0..rows.len())
            .filter(|i| !test.contains(i))
            .map(|i| rows[i])
            .collect();
        let held: Vec<&Line> = test.iter().map(|&i| rows[i]).collect();
        for (i, value) in test.iter().zip(score(&train, &held)) {
            scored[*i] = Some(value);
        }
    }
    scored.into_iter().flatten().collect()
}

/// The indices of each fold, by the rule the host grades with
/// (`lighthouse_protocol::stats::assign`). Empty folds are dropped.
fn folds(rows: &[&Line]) -> Vec<Vec<usize>> {
    let groups: Vec<&str> = rows.iter().map(|r| r.group.as_str()).collect();
    let assigned = assign(&groups);
    let mut folds = vec![Vec::new(); assigned.iter().max().map_or(0, |m| m + 1)];
    for (i, fold) in assigned.into_iter().enumerate() {
        folds[fold].push(i);
    }
    folds.retain(|fold| !fold.is_empty());
    folds
}

#[cfg(test)]
mod tests;
