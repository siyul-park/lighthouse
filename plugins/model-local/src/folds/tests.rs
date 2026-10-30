use std::collections::BTreeSet;

use super::*;

fn row(group: &str, label: f64) -> Line {
    Line {
        vector: vec![label],
        label,
        weight: 1.0,
        group: group.to_owned(),
        ..Line::default()
    }
}

#[test]
fn a_group_is_never_split_between_the_fit_and_the_test() {
    let rows: Vec<Line> = (0..30).map(|i| row(&format!("f{}", i % 7), 0.0)).collect();
    let refs: Vec<&Line> = rows.iter().collect();

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
    let refs: Vec<&Line> = rows.iter().collect();
    assert_eq!(folds(&refs), [vec![0, 1], vec![2], vec![3]]);
}

#[test]
fn one_group_is_scored_by_a_fit_on_itself() {
    let rows = [row("a", 0.0), row("a", 1.0)];
    let refs: Vec<&Line> = rows.iter().collect();
    let scored = out_of_fold(&refs, |train, test| vec![train.len(); test.len()]);
    assert_eq!(scored, [2, 2]);
}
