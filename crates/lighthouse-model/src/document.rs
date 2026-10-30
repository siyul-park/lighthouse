use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{API_VERSION, Error, Format, Position, documents};

/// A document of the project that is not code, such as a decision of a
/// catalog: a thing with a kind, a name and labels, written at a line of a
/// file. It is the first subject of the model that no language provider
/// states; the engine finds the documents itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub file: PathBuf,
    /// Where the name is written.
    pub at: Position,
    /// The document's `kind`, such as `Decision`.
    pub kind: String,
    /// The document's `metadata.name`.
    pub name: String,
    /// The document's `metadata.uid`, when it has one.
    pub uid: Option<String>,
    pub labels: BTreeMap<String, String>,
}

impl Document {
    /// The Lighthouse documents of `text`, the text of `file` (those of this
    /// model's `apiVersion`) with the place each name is written, and the
    /// errors of the parts of the text that could not be read: a part that
    /// fails does not hide the others. A name that cannot be found in its part
    /// stays at the top of the file and is reported as [`Error::Unplaced`].
    pub fn list(format: Format, file: &Path, text: &str) -> (Vec<Self>, Vec<Error>) {
        let label = file.display().to_string();
        let (mut found, mut errors) = (Vec::new(), Vec::new());
        for (first_line, part) in parts(format, text) {
            match documents(format, &label, part) {
                Ok(docs) => {
                    for mut document in docs.iter().filter_map(listing) {
                        document.file = file.to_owned();
                        match locate(format, part, &document.name) {
                            Some(at) => {
                                document.at = Position {
                                    line: first_line + at.line,
                                    col: at.col,
                                };
                            }
                            None => errors.push(Error::Unplaced {
                                path: label.clone(),
                                name: document.name.clone(),
                            }),
                        }
                        found.push(document);
                    }
                }
                Err(e) => errors.push(e),
            }
        }
        (found, errors)
    }
}

/// The text split into its documents, with the number of lines before each:
/// YAML documents are separated by `---`, the other formats hold one.
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

/// The identity `doc` declares, if it is a Lighthouse document; the file and
/// place are the caller's to fill in.
fn listing(doc: &Value) -> Option<Document> {
    if doc.get("apiVersion")?.as_str()? != API_VERSION {
        return None;
    }
    let metadata = doc.get("metadata")?;
    let labels = metadata
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
        file: PathBuf::new(),
        at: Position { line: 1, col: 1 },
        kind: doc.get("kind")?.as_str()?.to_owned(),
        name: metadata.get("name")?.as_str()?.to_owned(),
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
fn locate(format: Format, part: &str, name: &str) -> Option<Position> {
    let lines: Vec<&str> = part.lines().collect();
    let found = match format {
        Format::Yaml => yaml_name(&lines),
        Format::Toml => toml_name(&lines),
        Format::Json => json_name(&lines),
    }?;
    let col = lines[found].find(name).map_or(1, |c| c + 1);
    Some(Position {
        line: u32::try_from(found + 1).ok()?,
        col: u32::try_from(col).ok().unwrap_or(1),
    })
}

/// `metadata:` at the left margin, then the first `name:` at the indent of its
/// first key, before the block ends.
fn yaml_name(lines: &[&str]) -> Option<usize> {
    let start = lines.iter().position(|l| l.trim_end() == "metadata:")?;
    let mut depth = None;
    for (at, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let here = line.len() - line.trim_start().len();
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
