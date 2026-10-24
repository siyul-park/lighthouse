use lighthouse_plugin::Source;
use toml_edit::{Document, TableLike, Value};

use super::{EXPRESSION_FIELDS, host::*, yaml::quoted_scalar};

pub(super) fn toml_draft(source: &Source) -> Result<FileDraft, String> {
    let document =
        Document::parse(source.text).map_err(|error| format!("invalid TOML: {error}"))?;
    let mut draft = FileDraft::empty(source);
    let mut scalars = Vec::new();
    collect_scalars(
        document.as_table(),
        &mut Vec::new(),
        source.text,
        &mut scalars,
    )?;
    let doc = Doc {
        index: 0,
        kind: "Project".to_owned(),
        id: format!("Project:{}", source.file.path.display()),
    };
    for scalar in &scalars {
        add_snippet(&mut draft, &doc, scalar, scalar.key.as_str(), None, 0);
    }
    Ok(draft)
}

fn collect_scalars(
    table: &dyn TableLike,
    path: &mut Vec<String>,
    host: &str,
    scalars: &mut Vec<Scalar>,
) -> Result<(), String> {
    for (key, item) in table.iter() {
        path.push(key.to_owned());
        if let Some(child) = item.as_table_like() {
            collect_scalars(child, path, host, scalars)?;
        } else if expression_path(path)
            && let Some(value) = item.as_value().and_then(Value::as_str)
        {
            let range = item
                .span()
                .ok_or_else(|| format!("missing source span for {}", path.join(".")))?;
            let raw = host
                .get(range.clone())
                .ok_or_else(|| format!("invalid source span for {}", path.join(".")))?;
            scalars.push(Scalar {
                doc: 0,
                path: path.clone(),
                key: key.to_owned(),
                value: toml_string(host, raw, range.start, value.to_owned()).ok_or_else(|| {
                    format!("cannot map TOML string spans for {}", path.join("."))
                })?,
                source_start: range.start,
                source_end: range.end,
                item: None,
            });
        }
        path.pop();
    }
    Ok(())
}

fn expression_path(path: &[String]) -> bool {
    let prefix = if path_starts_with(path, &["models", "providers"]) {
        0
    } else if path_starts_with(path, &["spec", "models", "providers"]) {
        1
    } else {
        return false;
    };
    path.len() >= prefix + 5
        && path[prefix + 3] == "events"
        && EXPRESSION_FIELDS.contains(&path[prefix + 4].as_str())
}

fn toml_string(host: &str, raw: &str, source_start: usize, value: String) -> Option<MappedText> {
    if raw.starts_with("\"\"\"") || raw.starts_with("'''") {
        multiline_string(host, raw, source_start, value)
    } else if raw.starts_with('\'') || raw.starts_with('"') {
        Some(quoted_scalar(host, raw, source_start, value))
    } else {
        Some(MappedText::raw(host, raw, source_start))
    }
}

fn multiline_string(
    host: &str,
    raw: &str,
    source_start: usize,
    value: String,
) -> Option<MappedText> {
    let quote = *raw.as_bytes().first()?;
    if raw.len() < 6 || !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let end = raw.len().checked_sub(3)?;
    let mut at = 3;
    if raw[at..].starts_with("\r\n") {
        at += 2;
    } else if raw[at..].starts_with('\n') || raw[at..].starts_with('\r') {
        at += 1;
    }

    let mut out = 0;
    let mut starts = Vec::with_capacity(value.len());
    let mut ends = Vec::with_capacity(value.len());
    while at < end {
        if quote == b'"'
            && let Some(next) = continuation_end(raw, at, end)
        {
            at = next;
            continue;
        }
        let source_at = at;
        at += multiline_consumed(raw, at, quote, value.get(out..)?)?;
        if at > end {
            return None;
        }
        let character = value.get(out..)?.chars().next()?;
        starts.extend(std::iter::repeat_n(
            source_start + source_at,
            character.len_utf8(),
        ));
        ends.extend(std::iter::repeat_n(source_start + at, character.len_utf8()));
        out += character.len_utf8();
    }
    if out != value.len() || starts.len() != value.len() {
        return None;
    }
    let mapped_start = starts.first().copied().unwrap_or(source_start + 3);
    let mapped_end = ends.last().copied().unwrap_or(source_start + end);
    Some(MappedText {
        text: value,
        starts,
        ends,
        source_start: mapped_start,
        source_end: mapped_end,
        host: host.to_owned(),
    })
}

fn continuation_end(raw: &str, at: usize, end: usize) -> Option<usize> {
    if raw.as_bytes().get(at) != Some(&b'\\') {
        return None;
    }
    let newline = *raw.as_bytes().get(at + 1)?;
    if newline != b'\n' && newline != b'\r' {
        return None;
    }
    let mut next = at
        + if newline == b'\r' && raw.as_bytes().get(at + 2) == Some(&b'\n') {
            3
        } else {
            2
        };
    while next < end && raw.as_bytes()[next].is_ascii_whitespace() {
        next += 1;
    }
    Some(next)
}

fn multiline_consumed(raw: &str, at: usize, quote: u8, value: &str) -> Option<usize> {
    if quote == b'"' && raw.as_bytes()[at] == b'\\' {
        return Some(match raw.as_bytes().get(at + 1)? {
            b'u' => 6,
            b'U' => 10,
            _ => 2,
        });
    }
    if raw[at..].starts_with("\r\n") && value.starts_with('\n') {
        Some(2)
    } else {
        Some(raw[at..].chars().next()?.len_utf8())
    }
}
