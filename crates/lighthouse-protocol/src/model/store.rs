//! Where a project keeps what is stored by content: `sha256/<hex>` under a
//! directory, a dataset as lines (`.jsonl`) and a trained model as one
//! document (`.json`).

use std::path::{Path, PathBuf};

/// The datasets a trainer is given, under the project root.
pub const DATASETS_DIR: &str = ".lighthouse/datasets";

/// The trained models, under the project root. A trainer is told the directory
/// in the `dir` of its request; a model that serves a trained model finds it
/// under the root it was initialized with.
pub const MODELS_DIR: &str = ".lighthouse/models";

/// The directory under `dir` that holds the files named by their SHA-256.
pub fn content_dir(dir: &Path) -> PathBuf {
    dir.join("sha256")
}

/// The dataset of digest `hex` in the datasets directory `dir`.
pub fn dataset_path(dir: &Path, hex: &str) -> PathBuf {
    content_dir(dir).join(format!("{hex}.jsonl"))
}

/// The trained model of digest `hex` in the models directory `dir`.
pub fn artifact_path(dir: &Path, hex: &str) -> PathBuf {
    content_dir(dir).join(format!("{hex}.json"))
}
