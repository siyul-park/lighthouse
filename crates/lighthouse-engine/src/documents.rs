use std::{collections::BTreeMap, fs, path::Path};

use ignore::WalkBuilder;
use lighthouse_model::{Document, Position};
use lighthouse_resource::{API_VERSION, Format, documents};
use serde_json::Value;

use crate::engine::{IGNORE_FILE, Overlays};

/// Largest file looked at: configuration documents are small, and a data file
/// that happens to be YAML or JSON is not one.
const MAX_BYTES: u64 = 1 << 20;

/// Directories no command looks for documents in.
pub const SKIPPED_DIRS: [&str; 4] = [".git", "target", "node_modules", "testdata"];

/// The documents found, and what could not be settled about them.
#[derive(Default)]
pub(crate) struct Found {
    pub documents: Vec<Document>,
    pub notices: Vec<String>,
}

/// The Lighthouse documents of the project (decisions, packs, projects, ...)
/// found in its YAML, TOML and JSON files, with the line each name is written
/// on. Files the project ignores and the directories in [`SKIPPED_DIRS`] are
/// left out; a file that mentions Lighthouse but cannot be read or parsed is
/// reported, not skipped silently. `overlays` stand in for the files of the
/// same path.
pub(crate) fn find(root: &Path, overlays: &Overlays) -> Found {
    let walk = WalkBuilder::new(root)
        .require_git(false)
        .hidden(false)
        .add_custom_ignore_filename(IGNORE_FILE)
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| !SKIPPED_DIRS.contains(&name))
        })
        .build();
    let mut found = Found::default();
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
                    Err(e) => {
                        found.notices.push(format!(
                            "{}: not read as a document: {e}",
                            relative.display()
                        ));
                        continue;
                    }
                }
            }
            None => continue,
        };
        if text.contains("lighthouse/") {
            read(relative, format, &text, &mut found);
        }
    }
    found
}

/// The Lighthouse documents of one file.
fn read(file: &Path, format: Format, text: &str, found: &mut Found) {
    let label = file.display().to_string();
    for (first_line, part) in parts(format, text) {
        let docs = match documents(format, &label, part) {
            Ok(docs) => docs,
            Err(e) => {
                found
                    .notices
                    .push(format!("{label}: not read as a document: {e}"));
                continue;
            }
        };
        for doc in &docs {
            let Some(mut document) = document(doc, file) else {
                continue;
            };
            match locate(format, part, &document.name) {
                Some((line, col)) => {
                    document.at = Position {
                        line: first_line + line,
                        col,
                    };
                }
                None => found.notices.push(format!(
                    "{label}: could not find where `{}` is written; it is reported at the top of the file",
                    document.name
                )),
            }
            found.documents.push(document);
        }
    }
}

/// The text of a file split into its documents, with the number of lines
/// before each: YAML documents are separated by `---`, the other formats hold
/// one.
fn parts(format: Format, text: &str) -> Vec<(u32, &str)> {
    if format != Format::Yaml {
        return vec![(0, text)];
    }
    let mut starts = vec![(0usize, 0u32)];
    let (mut offset, mut line) = (0usize, 0u32);
    for content in text.split_inclusive('\n') {
        if offset > 0 && content.trim_end() == "---" {
            starts.push((offset, line));
        }
        offset += content.len();
        line += 1;
    }
    starts.push((text.len(), line));
    starts
        .windows(2)
        .map(|w| (w[0].1, &text[w[0].0..w[1].0]))
        .collect()
}

/// The Lighthouse document `doc` is.
fn document(doc: &Value, file: &Path) -> Option<Document> {
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
        at: Position { line: 1, col: 1 },
        kind: kind.to_owned(),
        name: name.to_owned(),
        uid: metadata
            .get("uid")
            .and_then(Value::as_str)
            .map(str::to_owned),
        labels,
    })
}

/// Where the `name` of the `metadata` of the document in `part` is written,
/// as a line (0-based within the part, plus one) and column; the key is looked
/// for inside the `metadata` block only, at the depth of its first key.
fn locate(format: Format, part: &str, name: &str) -> Option<(u32, u32)> {
    let lines: Vec<&str> = part.lines().collect();
    let found = match format {
        Format::Yaml => yaml_name(&lines),
        Format::Toml => toml_name(&lines),
        Format::Json => json_name(&lines),
    }?;
    let col = lines[found].find(name).map_or(1, |c| c + 1);
    Some((
        u32::try_from(found + 1).ok()?,
        u32::try_from(col).ok().unwrap_or(1),
    ))
}

/// `metadata:` at the left margin, then the first `name:` at the indent of its
/// first key, before the block ends.
fn yaml_name(lines: &[&str]) -> Option<usize> {
    let start = lines.iter().position(|l| l.trim_end() == "metadata:")?;
    let mut depth = None;
    for (at, line) in lines.iter().enumerate().skip(start + 1) {
        if !meaningful(line) {
            continue;
        }
        let here = indent(line);
        if here == 0 {
            return None;
        }
        let depth = *depth.get_or_insert(here);
        if here == depth && line.trim_start().starts_with("name:") {
            return Some(at);
        }
    }
    None
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn meaningful(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && !trimmed.starts_with('#')
}

/// A `[metadata]` table with a `name`, or an inline `metadata = { name = ... }`.
fn toml_name(lines: &[&str]) -> Option<usize> {
    if let Some(at) = lines.iter().position(|l| {
        let l = l.trim_start();
        l.starts_with("metadata") && l.contains('=') && l.contains("name")
    }) {
        return Some(at);
    }
    let start = lines.iter().position(|l| l.trim() == "[metadata]")?;
    lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .take_while(|(_, l)| !l.trim_start().starts_with('['))
        .find(|(_, l)| l.trim_start().starts_with("name"))
        .map(|(at, _)| at)
}

/// The first `"name"` after `"metadata"`.
fn json_name(lines: &[&str]) -> Option<usize> {
    let start = lines.iter().position(|l| l.contains("\"metadata\""))?;
    lines
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, l)| l.contains("\"name\""))
        .map(|(at, _)| at)
}
