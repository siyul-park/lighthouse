use std::collections::BTreeMap;

use lighthouse_model::hash;
use lighthouse_resource::Spec;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const KEYWORDS: [&str; 3] = ["MUST", "SHOULD", "MAY"];

/// The spec of the `SourceMap` kind: the normative lines of the documents a
/// catalog was written from, and which decision covers each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceMapSpec {
    pub sources: Vec<SourceLine>,
}

impl Spec for SourceMapSpec {
    const KIND: &'static str = "SourceMap";
}

/// One normative line of a source document and where it went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceLine {
    /// `<doc>#<heading-slug>-<content hash>`; moving or reordering a bullet
    /// keeps its ref, rewording it changes the ref.
    #[serde(rename = "ref")]
    pub reference: String,
    pub text: String,
    /// Ids of the decisions that cover the line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    /// Why no decision covers this line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bullet {
    pub reference: String,
    pub text: String,
}

struct Out<'a> {
    doc: &'a str,
    seen: BTreeMap<String, usize>,
    bullets: Vec<Bullet>,
}

impl<'a> Out<'a> {
    fn new(doc: &'a str) -> Self {
        Self {
            doc,
            seen: BTreeMap::new(),
            bullets: Vec::new(),
        }
    }

    fn finish(&mut self, item: Option<(String, String, bool)>) {
        let Some((heading, text, _)) = item else {
            return;
        };
        let heading = if heading.is_empty() { "doc" } else { &heading };
        let base = format!("{}#{heading}-{}", self.doc, hash::short(&text, 4));
        let n = self.seen.entry(base.clone()).or_default();
        *n += 1;
        let reference = if *n == 1 { base } else { format!("{base}-{n}") };
        self.bullets.push(Bullet { reference, text });
    }
}

/// Normative lines of a Markdown document: list items (`-`, `*`, numbered)
/// with their indented continuation lines, table body rows, and any other
/// single line holding an RFC keyword. Fenced code is skipped. Prose that
/// wraps over several lines is not joined; write such rules as list items.
pub(crate) fn extract(doc: &str, markdown: &str) -> Vec<Bullet> {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut out = Out::new(doc);
    let mut heading = String::new();
    let mut fence: Option<char> = None;
    let mut open: Option<(String, String, bool)> = None;
    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if let Some(marker) = fence_marker(line) {
            out.finish(open.take());
            fence = match fence {
                None => Some(marker),
                Some(f) if f == marker => None,
                other => other,
            };
            continue;
        }
        if fence.is_some() || line.is_empty() {
            out.finish(open.take());
            continue;
        }
        if let Some((_, text, true)) = open.as_mut()
            && raw.starts_with(char::is_whitespace)
            && list_item(line).is_none()
            && !line.starts_with(['|', '#'])
        {
            text.push(' ');
            text.push_str(line);
            continue;
        }
        out.finish(open.take());
        if line.starts_with('#') {
            heading = slug(line.trim_start_matches('#'));
            continue;
        }
        let next = lines.get(i + 1).map(|l| l.trim());
        open = normative(line, next).map(|(text, wraps)| (heading.clone(), text, wraps));
    }
    out.finish(open.take());
    out.bullets
}

pub(crate) fn has_keyword(line: &str) -> bool {
    line.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| KEYWORDS.contains(&word))
}

fn fence_marker(line: &str) -> Option<char> {
    if line.starts_with("```") {
        Some('`')
    } else if line.starts_with("~~~") {
        Some('~')
    } else {
        None
    }
}

/// The line's text, and whether indented lines after it continue it.
fn normative(line: &str, next: Option<&str>) -> Option<(String, bool)> {
    if line.starts_with('|') {
        let header = next.is_some_and(is_separator);
        return (!header && !is_separator(line)).then(|| (cells(line), false));
    }
    if let Some(item) = list_item(line) {
        return Some((item.to_owned(), true));
    }
    has_keyword(line).then(|| (line.to_owned(), false))
}

fn list_item(line: &str) -> Option<&str> {
    for marker in ["- ", "* "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest.trim());
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    line[digits..]
        .strip_prefix(". ")
        .filter(|_| digits > 0)
        .map(str::trim)
}

fn is_separator(line: &str) -> bool {
    line.starts_with('|') && line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn cells(row: &str) -> String {
    let inner = row.trim_start_matches('|').trim_end_matches('|');
    let cells: Vec<_> = inner.split('|').map(str::trim).collect();
    cells.join(" | ")
}

fn slug(heading: &str) -> String {
    let words: Vec<_> = heading
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    words.join("-")
}
