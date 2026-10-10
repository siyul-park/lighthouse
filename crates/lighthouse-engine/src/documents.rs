use std::{collections::BTreeMap, fs, path::Path};

use ignore::WalkBuilder;
use lighthouse_model::{Document, Position};
use lighthouse_resource::{API_VERSION, Format, documents};
use serde_json::Value;

use crate::engine::{IGNORE_FILE, Overlays};

/// Largest file looked at: configuration documents are small, and a data file
/// that happens to be YAML or JSON is not one.
const MAX_BYTES: u64 = 1 << 20;

/// The Lighthouse documents of the project (decisions, packs, projects, ...)
/// found in its YAML, TOML and JSON files, with the line each name is written
/// on. Files the project ignores are left out; a file that is not a Lighthouse
/// document is none of this function's business and is skipped without a word.
/// `overlays` stand in for the files of the same path.
pub(crate) fn find(root: &Path, overlays: &Overlays) -> Vec<Document> {
    let walk = WalkBuilder::new(root)
        .require_git(false)
        .hidden(false)
        .add_custom_ignore_filename(IGNORE_FILE)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    let mut found = Vec::new();
    for entry in walk.flatten() {
        let path = entry.path();
        let Some(format) = Format::of_path(path) else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let text = match overlays.get(relative) {
            Some(text) => text.clone(),
            None if entry
                .metadata()
                .is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES) =>
            {
                match fs::read_to_string(path) {
                    Ok(text) => text,
                    Err(_) => continue,
                }
            }
            None => continue,
        };
        if !text.contains("lighthouse/") {
            continue;
        }
        let label = relative.display().to_string();
        let Ok(docs) = documents(format, &label, &text) else {
            continue;
        };
        let mut cursor = 0;
        for doc in &docs {
            let Some(document) = document(doc, relative, &text, &mut cursor) else {
                continue;
            };
            found.push(document);
        }
    }
    found
}

/// The Lighthouse document `doc` is, written in `text` at or after the line
/// `cursor`.
fn document(doc: &Value, file: &Path, text: &str, cursor: &mut usize) -> Option<Document> {
    if doc.get("apiVersion")?.as_str()? != API_VERSION {
        return None;
    }
    let kind = doc.get("kind")?.as_str()?;
    let metadata = doc.get("metadata")?;
    let name = metadata.get("name")?.as_str()?;
    let labels: BTreeMap<String, String> = metadata
        .get("labels")
        .and_then(Value::as_object)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    Some(Document {
        file: file.to_owned(),
        at: locate(text, name, cursor),
        kind: kind.to_owned(),
        name: name.to_owned(),
        uid: metadata
            .get("uid")
            .and_then(Value::as_str)
            .map(str::to_owned),
        labels,
    })
}

/// Where `name` is written as a `name` key, looking from line `cursor` on
/// (documents come in file order); the top of the file when it cannot be
/// found.
fn locate(text: &str, name: &str, cursor: &mut usize) -> Position {
    for (at, line) in text.lines().enumerate().skip(*cursor) {
        let key = line.trim_start().trim_start_matches("- ");
        let is_name_key = ["name:", "name =", "\"name\":", "'name':"]
            .iter()
            .any(|key_text| key.starts_with(key_text));
        if let Some(col) = line.find(name).filter(|_| is_name_key) {
            *cursor = at + 1;
            return Position {
                line: u32::try_from(at + 1).unwrap_or(1),
                col: u32::try_from(col + 1).unwrap_or(1),
            };
        }
    }
    Position { line: 1, col: 1 }
}
