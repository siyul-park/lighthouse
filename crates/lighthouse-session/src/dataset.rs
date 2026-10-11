//! The one writer of training files: JSON Lines in OpenAI's supervised
//! fine-tuning format with the keys Lighthouse adds (mlops §2b), stored by
//! content in `.lighthouse/datasets/sha256/<hex>.jsonl`. Each line is
//! canonical JSON (RFC 8785) and the digest is the SHA-256 of the file, so the
//! same labels make the same file, and a model can name the dataset it was
//! trained on.

use std::{
    fs,
    path::{Path, PathBuf},
};

use lighthouse_model::{canonical, hash::sha256};
use lighthouse_plugin::{DecisionRef, LineKeys, Question, ResultKind, Subject, TrainingLine};
use serde_json::Value;

use crate::Result;

/// The directory datasets are stored in, under the project root.
pub const DIR: &str = ".lighthouse/datasets";

/// A labelled subject, ready to be a line.
pub(crate) struct Row {
    pub(crate) subject: Subject,
    pub(crate) label: ResultKind,
    pub(crate) weight: f64,
    pub(crate) key: String,
    pub(crate) attribution: Value,
}

/// A written dataset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dataset {
    /// `sha256:<hex>`: what a fine-tuning request calls its `training_file`.
    pub digest: String,
    pub path: PathBuf,
    pub lines: usize,
}

/// Writes the lines of `rows` for `decision`, ordered by key, and gives the
/// dataset.
pub(crate) fn write(root: &Path, decision: &DecisionRef, mut rows: Vec<Row>) -> Result<Dataset> {
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    let question = Question::of(decision);
    let mut text = String::new();
    for row in &rows {
        let group = if row.subject.file.is_empty() {
            row.key.as_str()
        } else {
            row.subject.file.as_str()
        };
        let line = TrainingLine::new(
            &question,
            &row.subject.state(),
            row.label,
            LineKeys {
                weight: row.weight,
                group,
                decision: &decision.id,
                key: &row.key,
                attribution: row.attribution.clone(),
            },
        );
        text.push_str(&canonical(&serde_json::to_value(&line)?));
        text.push('\n');
    }
    let hex = sha256(&text);
    let dir = root.join(DIR).join("sha256");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{hex}.jsonl"));
    fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Dataset {
        digest: format!("sha256:{hex}"),
        path,
        lines: rows.len(),
    })
}
