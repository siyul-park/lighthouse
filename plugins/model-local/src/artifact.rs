//! A trained model as a file: one JSON document stored by the digest of its
//! RFC 8785 canonical form, so that the same data makes the same file and a
//! reference to it cannot be satisfied by anything else.

use std::{fs, path::Path};

use base64::{Engine, engine::general_purpose::STANDARD};
use lighthouse_model::{
    canonical,
    hash::{digest, sha256},
    stats::FOLDS,
};
use serde_json::{Map, Value, json};

use crate::{
    candidates::Model,
    fit::Fitted,
    rows::{Dataset, sources},
};

/// The shape of the document.
const SCHEMA_VERSION: u32 = 1;
/// Changes whenever the features a model reads change.
const FEATURE_VERSION: u32 = 1;
/// Significant digits of every number in the document.
const DIGITS: i32 = 6;

/// What a model reads besides the row's vector: the vectors of the examples.
pub struct Examples<'a> {
    pub invalid: &'a [Vec<f64>],
    pub valid: &'a [Vec<f64>],
}

/// A model read back and verified.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub decision: String,
    /// `<name>@<dimension>`, as the embedder calls itself.
    pub embedder: String,
    pub model: Model,
    pub invalid: Vec<Vec<f64>>,
    pub valid: Vec<Vec<f64>>,
}

/// The card: what was trained on, what was tried and what was measured.
pub fn card(dataset: &Dataset, training_file: &str, fitted: &Fitted, trainer: &str) -> Value {
    let positives = dataset.lines.iter().filter(|l| l.label > 0.5).count();
    json!({
        "decision": dataset.decision,
        "trainer": trainer,
        "trainingFile": training_file,
        "rows": dataset.lines.len(),
        "positives": positives,
        "sources": sources(&dataset.lines),
        "candidates": fitted.candidates,
        "chosen": fitted.chosen,
        "metrics": {"brier": fitted.brier, "auroc": fitted.auroc},
        "band": fitted.band,
    })
}

/// The document of a model.
pub fn document(card: Value, embedder: &str, model: &Model, examples: &Examples) -> Value {
    let dim = embedder
        .rsplit_once('@')
        .and_then(|(_, dim)| dim.parse::<usize>().ok())
        .unwrap_or(0);
    let mut references = Map::from_iter([("dim".to_owned(), json!(dim))]);
    match model {
        Model::Rate(_) => {}
        Model::Knn(knn) => {
            references.insert("rows".to_owned(), encode(&knn.rows).into());
        }
        Model::Logistic(_) => {
            references.insert("invalid".to_owned(), encode(examples.invalid).into());
            references.insert("valid".to_owned(), encode(examples.valid).into());
        }
    }
    let mut document = json!({
        "schemaVersion": SCHEMA_VERSION,
        "card": card,
        "reproducibility": {
            "embedder": embedder,
            "featureVersion": FEATURE_VERSION,
            "seed": 0,
            "folds": FOLDS,
        },
        "model": model,
        "references": references,
    });
    round(&mut document);
    document
}

/// Stores the document under `dir` by its digest and gives the digest (hex).
pub fn write(dir: &Path, document: &Value) -> Result<String, String> {
    let text = canonical(document);
    let hex = sha256(&text);
    let store = dir.join("sha256");
    fs::create_dir_all(&store).map_err(|e| format!("{}: {e}", store.display()))?;
    let path = store.join(format!("{hex}.json"));
    fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(hex)
}

/// Reads the model with digest `hex` from the store under `dir`, and checks
/// that it is the one asked for.
pub fn load(dir: &Path, hex: &str) -> Result<Loaded, String> {
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(format!("`sha256:{hex}` is not a SHA-256 digest"));
    }
    let path = dir.join("sha256").join(format!("{hex}.json"));
    let text = fs::read_to_string(&path).map_err(|e| {
        format!(
            "{}: {e} (expected sha256:{hex}); run `lighthouse model rebuild`",
            path.display()
        )
    })?;
    let document: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let actual = digest(&document);
    if actual != hex {
        return Err(format!(
            "{}: its digest is sha256:{actual}, not the expected sha256:{hex}",
            path.display()
        ));
    }
    read(&document).map_err(|e| format!("{}: {e}", path.display()))
}

fn read(document: &Value) -> Result<Loaded, String> {
    let embedder = document["reproducibility"]["embedder"]
        .as_str()
        .ok_or("no embedder")?
        .to_owned();
    let dim = document["references"]["dim"].as_u64().unwrap_or(0) as usize;
    let mut model: Model =
        serde_json::from_value(document["model"].clone()).map_err(|e| format!("model: {e}"))?;
    let references = |name: &str| -> Result<Vec<Vec<f64>>, String> {
        match document["references"][name].as_str() {
            Some(text) => decode(text, dim),
            None => Ok(Vec::new()),
        }
    };
    if let Model::Knn(knn) = &mut model {
        knn.rows = references("rows")?;
        if knn.rows.len() != knn.labels.len() {
            return Err("the rows and labels of the neighbours differ".to_owned());
        }
    }
    Ok(Loaded {
        decision: document["card"]["decision"]
            .as_str()
            .ok_or("no decision")?
            .to_owned(),
        embedder,
        model,
        invalid: references("invalid")?,
        valid: references("valid")?,
    })
}

/// Vectors as little-endian `f32` in base64.
fn encode(vectors: &[Vec<f64>]) -> String {
    let bytes: Vec<u8> = vectors
        .iter()
        .flatten()
        .flat_map(|x| (*x as f32).to_le_bytes())
        .collect();
    STANDARD.encode(bytes)
}

fn decode(text: &str, dim: usize) -> Result<Vec<Vec<f64>>, String> {
    let bytes = STANDARD.decode(text).map_err(|e| e.to_string())?;
    if dim == 0 || bytes.len() % (4 * dim) != 0 {
        return Err("a vector has the wrong size".to_owned());
    }
    Ok(bytes
        .chunks_exact(4 * dim)
        .map(|row| {
            row.as_chunks::<4>()
                .0
                .iter()
                .map(|word| f64::from(f32::from_le_bytes(*word)))
                .collect()
        })
        .collect())
}

/// Rounds every non-integer number to [`DIGITS`] significant digits, so that
/// the last bits of a float cannot move the digest.
fn round(value: &mut Value) {
    match value {
        Value::Number(n) if n.is_f64() => {
            if let Some(x) = n
                .as_f64()
                .map(significant)
                .and_then(serde_json::Number::from_f64)
            {
                *value = Value::Number(x);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(round),
        Value::Object(map) => map.values_mut().for_each(round),
        _ => {}
    }
}

fn significant(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let shift = DIGITS - 1 - x.abs().log10().floor() as i32;
    let scale = 10f64.powi(shift.abs());
    if shift >= 0 {
        (x * scale).round() / scale
    } else {
        (x / scale).round() * scale
    }
}

#[cfg(test)]
mod tests {
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
        let path = dir.path().join("sha256").join(format!("{hex}.json"));
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
}
