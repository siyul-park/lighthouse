use super::*;
use crate::candidates::{Knn, Rate};
use crate::newton::Platt;

fn rate(rate: f64) -> Model {
    Model::Rate(Rate { rate })
}

fn none() -> Examples<'static> {
    Examples {
        invalid: &[],
        valid: &[],
    }
}

#[test]
fn numbers_keep_six_significant_digits() {
    assert_eq!(significant(0.123_456_789), 0.123_457);
    assert_eq!(significant(-0.000_123_456_7), -0.000_123_457);
    assert_eq!(significant(1_234_567.0), 1_234_570.0);
    assert_eq!(significant(0.0), 0.0);
}

#[test]
fn the_digest_is_that_of_the_canonical_form_and_the_file_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let document = document(
        json!({"decision": "d/x"}),
        "Fake@2",
        &rate(0.30000000000000004),
        &none(),
    );
    let hex = write(dir.path(), &document).unwrap();

    assert_eq!(hex, digest(&document));
    let loaded = load(dir.path(), &hex).unwrap();
    assert_eq!(loaded.decision, "d/x");
    assert_eq!(loaded.embedder, "Fake@2");
    assert_eq!(loaded.model, rate(0.3));
    // The same data is the same file.
    assert_eq!(write(dir.path(), &document).unwrap(), hex);
}

#[test]
fn neighbours_keep_their_vectors_among_the_references() {
    let dir = tempfile::tempdir().unwrap();
    let knn = Model::Knn(Knn {
        k: 5,
        rate: 0.5,
        labels: vec![1.0, 0.0],
        weights: vec![1.0, 1.0],
        rows: vec![vec![0.5, 0.25], vec![-1.0, 0.0]],
        platt: Platt::IDENTITY,
    });
    let hex = write(
        dir.path(),
        &document(json!({"decision": "d/x"}), "Fake@2", &knn, &none()),
    )
    .unwrap();

    assert_eq!(load(dir.path(), &hex).unwrap().model, knn);
}

#[test]
fn a_file_that_is_not_what_its_name_says_is_refused_with_both_digests() {
    let dir = tempfile::tempdir().unwrap();
    let document = document(json!({"decision": "d/x"}), "Fake@2", &rate(0.3), &none());
    let hex = write(dir.path(), &document).unwrap();
    let path = artifact_path(dir.path(), &hex);
    let tampered = fs::read_to_string(&path).unwrap().replace("0.3", "0.9");
    fs::write(&path, tampered).unwrap();

    let error = load(dir.path(), &hex).unwrap_err();
    assert!(
        error.contains(&format!("not the expected sha256:{hex}")),
        "{error}"
    );
    assert!(error.contains(&path.display().to_string()), "{error}");
}

#[test]
fn a_missing_file_names_the_path_and_the_digest() {
    let dir = tempfile::tempdir().unwrap();
    let hex = "ab".repeat(32);
    let error = load(dir.path(), &hex).unwrap_err();

    assert!(error.contains(&format!("expected sha256:{hex}")), "{error}");
    assert!(
        load(dir.path(), "../x")
            .unwrap_err()
            .contains("not a SHA-256")
    );
}
