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

use lighthouse_model::Result;
use lighthouse_plugin::{
    DATASETS_DIR, DecisionRef, LineKeys, Question, Subject, TrainingLine, dataset_path,
};
use lighthouse_protocol::{canonical, hash::sha256};
use serde_json::Value;

use crate::labels::Source;

/// A labelled subject, ready to be a line.
pub(crate) struct Row {
    pub(crate) subject: Subject,
    /// Whether the subject fails the decision.
    pub(crate) fail: bool,
    pub(crate) source: Source,
    pub(crate) key: String,
    pub(crate) attribution: Value,
}

/// A line of a stored dataset: what the cross-fit needs to place it, and its
/// text, which is canonical already.
pub(crate) struct Line {
    pub(crate) key: String,
    pub(crate) group: String,
    pub(crate) text: String,
}

/// Writes the lines of `rows` for `decision`, ordered by key, and gives the
/// digest of the dataset.
pub(crate) fn write(root: &Path, decision: &DecisionRef, mut rows: Vec<Row>) -> Result<String> {
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    let question = Question::new(decision);
    let mut lines = Vec::new();
    for row in &rows {
        let group = if row.subject.state.file.is_empty() {
            row.key.as_str()
        } else {
            row.subject.state.file.as_str()
        };
        let line = TrainingLine::new(
            &question,
            &row.subject.state,
            row.fail,
            LineKeys {
                weight: row.source.weight(),
                group,
                decision: &decision.id,
                key: &row.key,
                attribution: row.attribution.clone(),
            },
        );
        lines.push(canonical(&serde_json::to_value(&line)?));
    }
    write_lines(root, &lines)
}

/// Writes canonical `lines` as a dataset and gives its digest: the one place a
/// training file is made, so that equal lines are one file.
pub(crate) fn write_lines(root: &Path, lines: &[String]) -> Result<String> {
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    let hex = sha256(&text);
    let path = dataset_path(&root.join(DATASETS_DIR), &hex);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!("sha256:{hex}"))
}

/// The path of the dataset `digest` (`sha256:<hex>`) names.
pub(crate) fn path_of(root: &Path, digest: &str) -> PathBuf {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    dataset_path(&root.join(DATASETS_DIR), hex)
}

/// The lines of the dataset `digest` names, or `None` when the store does not
/// hold it.
pub(crate) fn read(root: &Path, digest: &str) -> Result<Option<Vec<Line>>> {
    let path = path_of(root, digest);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(None);
    };
    let mut lines = Vec::new();
    for text in text.lines() {
        let line: Value =
            serde_json::from_str(text).map_err(|e| format!("{}: {e}", path.display()))?;
        let field = |name: &str| line.get(name).and_then(Value::as_str).unwrap_or_default();
        lines.push(Line {
            key: field("key").to_owned(),
            group: field("group").to_owned(),
            text: text.to_owned(),
        });
    }
    Ok(Some(lines))
}
