use lighthouse_model::stats::{FOLDS, assign, fold, wilson};

#[test]
fn the_fold_of_a_group_is_its_hash_modulo_the_folds() {
    // sha256("a") starts with ca978112, which is 3398926610.
    assert_eq!(fold("a", FOLDS), 0);
    // sha256("b") starts with 3e23e816, which is 1042540566.
    assert_eq!(fold("b", FOLDS), 1);
}

#[test]
fn groups_enough_are_folded_by_hash() {
    let groups = ["a", "b", "c", "d", "e", "a"];
    let folds = assign(&groups);
    assert_eq!(folds.len(), groups.len());
    for (group, at) in groups.iter().zip(&folds) {
        assert_eq!(*at, fold(group, FOLDS));
    }
}

#[test]
fn fewer_groups_than_folds_are_one_fold_each() {
    assert_eq!(assign(&["a", "a", "b", "c"]), [0, 0, 1, 2]);
    assert_eq!(assign(&["z", "a", "z"]), [1, 0, 1]);
}

#[test]
fn wilson_matches_the_published_example_and_bounds_the_extremes() {
    // 81 of 263: the textbook interval is about 0.2553 to 0.3662.
    let (low, high) = wilson(81.0, 263.0);
    assert!((low - 0.2553).abs() < 0.001, "{low}");
    assert!((high - 0.3662).abs() < 0.001, "{high}");

    assert_eq!(wilson(0.0, 10.0).0, 0.0);
    assert!((wilson(10.0, 10.0).1 - 1.0).abs() < 1e-12);
    // Weights need not be whole.
    let (low, high) = wilson(0.7, 1.4);
    assert!(low < 0.5 && 0.5 < high);
}
