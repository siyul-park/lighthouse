use lighthouse_store::{NewFix, Store};

#[test]
fn a_recorded_fix_is_kept_locally_and_never_enters_the_decision_log() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let fix = NewFix {
        fingerprint: "f1".to_owned(),
        rule_id: "design/declaration-groups".to_owned(),
        fixer: "design/declaration-groups".to_owned(),
        safety: "safe".to_owned(),
        description: "Declarations follow ownership groups".to_owned(),
        files: vec!["a.go".to_owned()],
        commit: Some("abc".to_owned()),
        lighthouse_version: "0.1.0".to_owned(),
    };

    let recorded = store.record_fix(&fix).unwrap();

    let fixes = store.fixes("f1").unwrap();
    assert_eq!(fixes, [recorded]);
    assert_eq!(fixes[0].files, ["a.go"]);
    assert!(store.fixes("other").unwrap().is_empty());
    assert!(!Store::log_path_in(dir.path()).exists());
}
