use lighthouse_model::{FOLDS, assign, fold};

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
