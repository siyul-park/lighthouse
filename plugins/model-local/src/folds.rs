//! The rows learners fit, how they are split into folds by group (so that
//! subjects of one group, one file, are never in both the part a model is
//! fitted on and the part it is judged on), and how a probability on them is
//! judged.

use std::collections::BTreeSet;

use lighthouse_model::hash::short;

/// The number of folds, when there are groups enough.
pub const FOLDS: usize = 5;

/// One labelled subject, embedded.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub vector: Vec<f64>,
    /// Maximum and mean cosine to the invalid examples, then to the valid
    /// ones; 0 when a side has none.
    pub sims: [f64; 4],
    /// 1 for a fail, 0 for a pass.
    pub label: f64,
    pub weight: f64,
    pub group: String,
}

/// The weighted mean of the squared distance of `p`, a probability of each row
/// failing, from its label.
pub fn brier(p: &[f64], rows: &[&Row]) -> f64 {
    let (error, weight) = squared_errors(p, rows)
        .iter()
        .zip(rows)
        .fold((0.0, 0.0), |(e, w), (x, r)| {
            (e + r.weight * x, w + r.weight)
        });
    error / weight
}

/// The squared distance of each probability from its row's label.
pub fn squared_errors(p: &[f64], rows: &[&Row]) -> Vec<f64> {
    p.iter()
        .zip(rows)
        .map(|(p, r)| (p - r.label).powi(2))
        .collect()
}

/// What `score` gives for every row when each is left out with its fold:
/// `score(train, test)` fits on `train` and gives one value per row of `test`.
/// With a single group there is nothing to leave out, and the rows are scored
/// by a fit on themselves.
pub fn out_of_fold<T>(rows: &[&Row], score: impl Fn(&[&Row], &[&Row]) -> Vec<T>) -> Vec<T> {
    let tests = folds(rows);
    if tests.len() < 2 {
        return score(rows, rows);
    }
    let mut scored: Vec<Option<T>> = rows.iter().map(|_| None).collect();
    for test in tests {
        let train: Vec<&Row> = (0..rows.len())
            .filter(|i| !test.contains(i))
            .map(|i| rows[i])
            .collect();
        let held: Vec<&Row> = test.iter().map(|&i| rows[i]).collect();
        for (i, value) in test.iter().zip(score(&train, &held)) {
            scored[*i] = Some(value);
        }
    }
    scored.into_iter().flatten().collect()
}

/// The indices of each fold: by the hash of the group (stable, whatever else
/// is in the data) and, when there are fewer groups than [`FOLDS`], one group
/// a fold. Empty folds are dropped.
fn folds(rows: &[&Row]) -> Vec<Vec<usize>> {
    let groups: BTreeSet<&str> = rows.iter().map(|r| r.group.as_str()).collect();
    let by_group = groups.len() < FOLDS;
    let fold_of = |group: &str| {
        if by_group {
            groups.iter().position(|g| *g == group).unwrap_or(0)
        } else {
            hashed(group)
        }
    };
    let mut folds = vec![Vec::new(); if by_group { groups.len() } else { FOLDS }];
    for (i, row) in rows.iter().enumerate() {
        folds[fold_of(&row.group)].push(i);
    }
    folds.retain(|fold| !fold.is_empty());
    folds
}

/// The fold of a group: the first 8 hex digits of its SHA-256, modulo the
/// number of folds.
fn hashed(group: &str) -> usize {
    u32::from_str_radix(&short(group, 4), 16).unwrap_or(0) as usize % FOLDS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(group: &str, label: f64) -> Row {
        Row {
            vector: vec![label],
            sims: [0.0; 4],
            label,
            weight: 1.0,
            group: group.to_owned(),
        }
    }

    #[test]
    fn a_group_is_never_split_between_the_fit_and_the_test() {
        let rows: Vec<Row> = (0..30).map(|i| row(&format!("f{}", i % 7), 0.0)).collect();
        let refs: Vec<&Row> = rows.iter().collect();

        let scored = out_of_fold(&refs, |train, test| {
            let held: BTreeSet<&str> = test.iter().map(|r| r.group.as_str()).collect();
            assert!(train.iter().all(|r| !held.contains(r.group.as_str())));
            test.iter().map(|_| train.len()).collect()
        });

        assert_eq!(scored.len(), 30);
        assert!(scored.iter().all(|&n| n < 30));
    }

    #[test]
    fn fewer_groups_than_folds_leave_one_group_out() {
        let rows = [row("a", 0.0), row("a", 1.0), row("b", 0.0), row("c", 1.0)];
        let refs: Vec<&Row> = rows.iter().collect();
        assert_eq!(folds(&refs), [vec![0, 1], vec![2], vec![3]]);
    }

    #[test]
    fn the_fold_of_a_group_is_its_hash_modulo_the_folds() {
        // sha256("a") starts with ca978112, which is 3398926610.
        assert_eq!(hashed("a"), 0);
        // sha256("b") starts with 3e23e816, which is 1042540566.
        assert_eq!(hashed("b"), 1);
    }

    #[test]
    fn one_group_is_scored_by_a_fit_on_itself() {
        let rows = [row("a", 0.0), row("a", 1.0)];
        let refs: Vec<&Row> = rows.iter().collect();
        let scored = out_of_fold(&refs, |train, test| vec![train.len(); test.len()]);
        assert_eq!(scored, [2, 2]);
    }
}
