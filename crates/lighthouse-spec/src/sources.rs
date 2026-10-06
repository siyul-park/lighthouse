use std::collections::BTreeMap;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// One normative line of a source document and where it went.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    /// `<doc>#<heading-slug>-<content hash>`; moving or reordering a bullet
    /// keeps its ref, rewording it changes the ref.
    #[serde(rename = "ref")]
    pub reference: String,
    pub text: String,
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Why no pattern covers this line.
    pub omitted: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bullet {
    pub reference: String,
    pub text: String,
}

const KEYWORDS: [&str; 3] = ["MUST", "SHOULD", "MAY"];

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
        let base = format!("{}#{heading}-{}", self.doc, hash(&text));
        let n = self.seen.entry(base.clone()).or_default();
        *n += 1;
        let reference = if *n == 1 { base } else { format!("{base}-{n}") };
        self.bullets.push(Bullet { reference, text });
    }
}

fn hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
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

pub(crate) fn has_keyword(line: &str) -> bool {
    line.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| KEYWORDS.contains(&word))
}

fn slug(heading: &str) -> String {
    let words: Vec<_> = heading
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    words.join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(markdown: &str) -> Vec<String> {
        extract("d", markdown).into_iter().map(|b| b.text).collect()
    }

    #[test]
    fn takes_list_items_numbered_items_and_table_body_rows() {
        let markdown =
            "## Part\n\n- first\n* second\n\n1. numbered\n\n| H | V |\n| --- | --- |\n| a | b |\n";
        assert_eq!(texts(markdown), ["first", "second", "numbered", "a | b"]);
    }

    #[test]
    fn joins_indented_continuation_lines_into_the_bullet() {
        let markdown = "## Part\n\n- first line\n  wraps here\n    and here\n- next\n";
        assert_eq!(texts(markdown), ["first line wraps here and here", "next"]);
    }

    #[test]
    fn nested_indented_items_stay_separate() {
        assert_eq!(texts("- outer\n  - inner\n"), ["outer", "inner"]);
    }

    #[test]
    fn takes_single_line_prose_only_with_an_rfc_keyword() {
        let markdown = "Plain prose.\n\nThis MUST be kept.\n\nShould not count.\n";
        assert_eq!(texts(markdown), ["This MUST be kept."]);
    }

    #[test]
    fn skips_backtick_and_tilde_fences() {
        let markdown = "```\n- a\n```\n~~~\n- b\n- c MUST\n~~~\n- d\n";
        assert_eq!(texts(markdown), ["d"]);
    }

    #[test]
    fn refs_follow_content_not_position() {
        let a = extract("d", "## H\n- one\n- two\n");
        let b = extract("d", "## H\n- two\n- one\n");
        let find = |bullets: &[Bullet], text: &str| {
            bullets
                .iter()
                .find(|b| b.text == text)
                .unwrap()
                .reference
                .clone()
        };
        assert_eq!(find(&a, "one"), find(&b, "one"));
        assert_ne!(find(&a, "one"), find(&a, "two"));
        assert!(find(&a, "one").starts_with("d#h-"));
    }

    #[test]
    fn repeated_text_under_one_heading_gets_distinct_refs() {
        let refs: Vec<_> = extract("d", "## H\n- same\n- same\n")
            .into_iter()
            .map(|b| b.reference)
            .collect();
        assert_ne!(refs[0], refs[1]);
    }
}
