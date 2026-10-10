//! Gives every `Decision` a `metadata.uid`: a UUID v4 assigned once, written
//! into the document and never derived from the name (Kubernetes
//! `metadata.uid`). A decision that has one is left as it is, so running the
//! migration twice changes nothing and a rename never touches a uid.
//!
//! A YAML file is edited as text, one line inserted after `name:`, so that
//! comments and the layout of the document stay; a TOML or JSON decision of a
//! project's `.lighthouse/decisions` is written back in its own format.

use std::{
    fs,
    path::{Path, PathBuf},
};

use lighthouse_resource::{Format, documents, new_uid};
use serde_json::Value;

use super::{Action, Plan, file_name, render_config};
use crate::Result;

/// What assigning uids to one file came to.
enum Assigned {
    Unchanged,
    Text(String),
    /// The name of a decision the uid could not be placed in.
    Unplaced(String),
}

/// Plans the uids of the decisions in `files` (and in the files `plan`
/// already writes). A document the migration could not place a uid in is
/// reported as a warning and left as it is.
pub(super) fn assign(files: &[PathBuf], plan: &mut Plan) -> Result<()> {
    let mut candidates: Vec<PathBuf> = files.iter().filter(|f| decision_file(f)).cloned().collect();
    candidates.extend(
        plan.actions
            .iter()
            .filter(|(path, action)| matches!(action, Action::Write(_)) && decision_file(path))
            .map(|(path, _)| path.clone()),
    );
    candidates.sort();
    candidates.dedup();
    for path in candidates {
        let text = match plan.actions.get(&path) {
            Some(Action::Write(text)) => text.clone(),
            Some(Action::Remove) => continue,
            None => fs::read_to_string(&path)?,
        };
        let label = path.display().to_string();
        let format = Format::of_path(&path).unwrap_or(Format::Yaml);
        let assigned = match format {
            Format::Yaml => yaml(&text, &label)?,
            Format::Toml | Format::Json => data(&text, &path, format, &label)?,
        };
        match assigned {
            Assigned::Unchanged => {}
            Assigned::Text(text) => {
                plan.actions.insert(path, Action::Write(text));
            }
            Assigned::Unplaced(name) => plan.kept.push(format!(
                "{label}: no uid was added to `{name}`; add `uid: {}` under `metadata` by hand",
                new_uid()
            )),
        }
    }
    Ok(())
}

/// A file that may hold decisions: a YAML file of a catalog, or any document
/// of a project's `.lighthouse/decisions`.
fn decision_file(path: &Path) -> bool {
    let local = path
        .components()
        .map(|c| c.as_os_str())
        .collect::<Vec<_>>()
        .windows(2)
        .any(|w| w[0] == ".lighthouse" && w[1] == "decisions");
    match Format::of_path(path) {
        Some(Format::Yaml) => !matches!(file_name(path).as_str(), "pack.yaml" | "sources.yaml"),
        Some(_) => local,
        None => false,
    }
}

fn lacks_uid(doc: &Value) -> Option<&str> {
    if doc.get("kind").and_then(Value::as_str) != Some("Decision") {
        return None;
    }
    let metadata = doc.get("metadata")?;
    if metadata.get("uid").is_some() {
        return None;
    }
    metadata.get("name")?.as_str()
}

/// `text`, a YAML file of any number of documents, with a uid in every
/// decision that lacks one.
fn yaml(text: &str, label: &str) -> Result<Assigned> {
    let mut out = String::with_capacity(text.len() + 64);
    let mut changed = false;
    for chunk in chunks(text) {
        let docs = documents(Format::Yaml, label, chunk)?;
        let Some(name) = docs.first().and_then(lacks_uid) else {
            out.push_str(chunk);
            continue;
        };
        match insert(chunk, &new_uid()) {
            Some(with) => {
                out.push_str(&with);
                changed = true;
            }
            None => return Ok(Assigned::Unplaced(name.to_owned())),
        }
    }
    Ok(if changed {
        Assigned::Text(out)
    } else {
        Assigned::Unchanged
    })
}

/// The documents of a YAML text, each with the `---` line that opens it.
fn chunks(text: &str) -> Vec<&str> {
    let mut starts = vec![0];
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if offset > 0 && line.trim_end() == "---" {
            starts.push(offset);
        }
        offset += line.len();
    }
    starts.push(text.len());
    starts.windows(2).map(|w| &text[w[0]..w[1]]).collect()
}

/// `chunk` with `uid: <uid>` on the line after the `name:` of its `metadata`
/// block; `None` when `metadata` is not written as a block with a `name:` line.
fn insert(chunk: &str, uid: &str) -> Option<String> {
    let lines: Vec<&str> = chunk.split_inclusive('\n').collect();
    let metadata = lines.iter().position(|l| l.trim_end() == "metadata:")?;
    for (offset, line) in lines.iter().enumerate().skip(metadata + 1) {
        let body = line.trim_start();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        if !line.starts_with(' ') {
            return None;
        }
        if body.starts_with("name:") {
            let indent = &line[..line.len() - body.len()];
            let mut out: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
            if !out[offset].ends_with('\n') {
                out[offset].push('\n');
            }
            out.insert(offset + 1, format!("{indent}uid: {uid}\n"));
            return Some(out.concat());
        }
    }
    None
}

/// A TOML or JSON decision, written back in its own format with a uid.
fn data(text: &str, path: &Path, format: Format, label: &str) -> Result<Assigned> {
    let mut docs = documents(format, label, text)?;
    if docs.len() != 1 {
        return Ok(Assigned::Unchanged);
    }
    let Some(name) = docs.first().and_then(lacks_uid).map(str::to_owned) else {
        return Ok(Assigned::Unchanged);
    };
    let doc = &mut docs[0];
    let Some(metadata) = doc.get_mut("metadata").and_then(Value::as_object_mut) else {
        return Ok(Assigned::Unplaced(name));
    };
    metadata.insert("uid".to_owned(), Value::String(new_uid()));
    Ok(Assigned::Text(render_config(format, path, doc)?))
}
