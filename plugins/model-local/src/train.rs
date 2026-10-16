//! `lighthouse/train`: a training file of the host's dataset store in, a
//! model in the host's model store out. Nothing is read but the file, so the
//! same file gives the same model.

use std::{fs, path::Path};

use lighthouse_model::hash::sha256;
use lighthouse_protocol::{CreateFineTuningJobRequest, dataset_path};
use serde_json::Value;

use crate::{
    artifact::{self, Examples},
    candidates::{MIN_PER_CLASS, similarities},
    embed::{DEFAULT, Embedders},
    fit::{Fitted, fit},
    rows::{self, Dataset, Line},
};

/// What a fail and a pass of the band must be right about, unless asked.
const PRECISION: f64 = 0.9;

/// A model that is stored.
pub struct Trained {
    /// The digest of its artifact (hex).
    pub hex: String,
    pub fitted: Fitted,
    /// `<name>@<dimension>`, or the name alone when nothing was embedded.
    pub embedder: String,
}

/// The rows of a dataset, the vectors of its examples and the embedder.
struct Featured {
    rows: Vec<Line>,
    invalid: Vec<Vec<f64>>,
    valid: Vec<Vec<f64>>,
    embedder: String,
}

pub fn run(
    request: &CreateFineTuningJobRequest,
    embedders: &mut Embedders,
) -> Result<Trained, String> {
    let dataset = read(request)?;
    let name = request
        .options
        .get("embedder")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT);
    let precision = request
        .options
        .get("precision")
        .and_then(Value::as_f64)
        .unwrap_or(PRECISION);
    let Featured {
        rows,
        invalid,
        valid,
        embedder,
    } = featurize(&dataset, name, embedders)?;
    let fitted = fit(&rows, precision);
    let card = artifact::card(&dataset, &request.training_file, &fitted, trainer());
    let document = artifact::document(
        card,
        &embedder,
        &fitted.model,
        &Examples {
            invalid: &invalid,
            valid: &valid,
        },
    );
    let dir = metadata(request, "dir")?;
    let hex = artifact::write(Path::new(dir), &document)?;
    Ok(Trained {
        hex,
        fitted,
        embedder,
    })
}

/// Who trained, as the card says.
fn trainer() -> &'static str {
    concat!("local/learned@", env!("CARGO_PKG_VERSION"))
}

fn metadata<'a>(request: &'a CreateFineTuningJobRequest, key: &str) -> Result<&'a str, String> {
    request
        .metadata
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| format!("the request has no `{key}` in its metadata"))
}

/// The training file, checked against the digest it is named by.
fn read(request: &CreateFineTuningJobRequest) -> Result<Dataset, String> {
    let hex = request
        .training_file
        .strip_prefix("sha256:")
        .ok_or("the training file is not a `sha256:<hex>` digest")?;
    let path = dataset_path(Path::new(metadata(request, "datasets")?), hex);
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("the training file {}: {e}", path.display()))?;
    if sha256(&text) != hex {
        return Err(format!(
            "{} is not the training file sha256:{hex}",
            path.display()
        ));
    }
    rows::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The rows of the dataset and the vectors of its examples. Nothing is
/// embedded when the rate is all the labels allow, so that a small dataset
/// does not wait for weights.
fn featurize(dataset: &Dataset, name: &str, embedders: &mut Embedders) -> Result<Featured, String> {
    let fails = dataset.lines.iter().filter(|l| l.label > 0.5).count();
    let enough = fails >= MIN_PER_CLASS && dataset.lines.len() - fails >= MIN_PER_CLASS;
    let sized = |vectors: Vec<Vec<f32>>| -> Vec<Vec<f64>> {
        vectors
            .into_iter()
            .map(|v| v.into_iter().map(f64::from).collect())
            .collect()
    };
    let (vectors, invalid, valid, embedder) = if enough {
        let embedder = embedders.get(name)?;
        let texts: Vec<String> = dataset
            .lines
            .iter()
            .map(|l| l.text.clone())
            .chain(dataset.invalid.iter().cloned())
            .chain(dataset.valid.iter().cloned())
            .collect();
        let mut vectors = sized(embedder.vectors(&texts)?);
        let valid = vectors.split_off(vectors.len() - dataset.valid.len());
        let invalid = vectors.split_off(vectors.len() - dataset.invalid.len());
        (vectors, invalid, valid, embedder.id())
    } else {
        let none = || vec![Vec::new(); dataset.lines.len()];
        (none(), Vec::new(), Vec::new(), name.to_owned())
    };
    let rows = dataset
        .lines
        .iter()
        .zip(vectors)
        .map(|(line, vector)| Line {
            sims: similarities(&vector, &invalid, &valid),
            vector,
            ..line.clone()
        })
        .collect();
    Ok(Featured {
        rows,
        invalid,
        valid,
        embedder,
    })
}
