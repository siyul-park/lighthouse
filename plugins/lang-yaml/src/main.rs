use std::{collections::BTreeMap, fs, path::PathBuf};

use lighthouse_protocol::{
    Conventions, ErrorObject, FileInfo, Fragment, Handler, Incomplete, IndexInput, IndexParams,
    IndexResult, InitializeParams, InitializeResult, Mapping, ProviderManifest, Source,
    TextMapSegment, pointer_child,
};
use saphyr::{LoadableYamlNode, MarkedYaml, YamlData};
use serde_json::Value;

const LANGUAGE: &str = "yaml";
const PLUGIN: &str = "lang-yaml";

struct MappedScalar {
    text: String,
    map: SourceOffsets,
}

pub struct Provider {
    version: String,
}

impl Handler for Provider {
    fn initialize(&mut self, _: InitializeParams) -> Result<InitializeResult, ErrorObject> {
        Ok(InitializeResult::new(
            PLUGIN,
            &self.version,
            vec![ProviderManifest {
                id: LANGUAGE.to_owned(),
                globs: vec!["**/*.yaml".to_owned(), "**/*.yml".to_owned()],
                priority: 20,
                fallback: false,
                conventions: Conventions::default(),
                capabilities: vec!["source-ast".to_owned()],
            }],
            Vec::new(),
        ))
    }

    fn index(&mut self, params: IndexParams) -> Result<IndexResult, ErrorObject> {
        let mut result = IndexResult::default();
        let overlays: BTreeMap<&str, &str> = params
            .context
            .overlays
            .iter()
            .flatten()
            .map(|overlay| (overlay.path.as_str(), overlay.text.as_str()))
            .collect();
        for input in &params.inputs {
            let file = match input {
                IndexInput::File {
                    file,
                    source_ast: true,
                } => file,
                IndexInput::File { .. } => continue,
                IndexInput::Regions { regions } => {
                    result.incomplete.push(Incomplete {
                        path: Some(regions.path.clone()),
                        reason: format!(
                            "{LANGUAGE} provider does not analyze selected source regions"
                        ),
                    });
                    continue;
                }
            };
            let path = PathBuf::from(&file.path);
            let owned;
            let text = if let Some(text) = overlays.get(file.path.as_str()) {
                *text
            } else {
                owned = match fs::read_to_string(PathBuf::from(&params.project.root).join(&path)) {
                    Ok(text) => text,
                    Err(error) => {
                        result.incomplete.push(Incomplete {
                            path: Some(file.path.clone()),
                            reason: format!("cannot read YAML host document: {error}"),
                        });
                        continue;
                    }
                };
                &owned
            };
            let byte_offsets = char_byte_offsets(text);
            match MarkedYaml::load_from_str(text) {
                Ok(documents) => {
                    let mut source_map = BTreeMap::from([
                        (String::new(), range_entry(0, text.len())),
                        ("/documents".to_owned(), range_entry(0, text.len())),
                    ]);
                    let mut values = Vec::with_capacity(documents.len());
                    let mut failed = None;
                    for (index, root) in documents.iter().enumerate() {
                        let pointer = pointer_child("/documents", &index.to_string());
                        match yaml_value(root, &pointer, text, &byte_offsets, &mut source_map) {
                            Ok(value) => values.push(value),
                            Err(reason) => {
                                failed = Some(reason);
                                break;
                            }
                        }
                    }
                    if let Some(reason) = failed {
                        result.incomplete.push(Incomplete {
                            path: Some(file.path.clone()),
                            reason: format!("cannot expose YAML AST: {reason}"),
                        });
                        continue;
                    }
                    result.sources.push(Source {
                        path: file.path.clone(),
                        ast: serde_json::json!({
                            "kind": "Stream",
                            "documents": values,
                        }),
                        mapping: source_map,
                    });
                    result.fragments.push(Fragment {
                        file: FileInfo {
                            path: file.path.clone(),
                            ..FileInfo::default()
                        },
                        ..Fragment::default()
                    });
                }
                Err(error) => result.incomplete.push(Incomplete {
                    path: Some(file.path.clone()),
                    reason: format!("invalid YAML: {error}"),
                }),
            }
        }
        Ok(result)
    }
}

#[derive(Clone)]
struct SourceOffsets {
    starts: Vec<u32>,
    ends: Vec<u32>,
}

fn char_byte_offsets(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(offset, _)| offset)
        .chain(std::iter::once(text.len()))
        .collect()
}

fn range_entry(start: usize, end: usize) -> Mapping {
    Mapping {
        range: Some([start as u32, end as u32]),
        text_pointer: None,
        text_map: None,
    }
}

fn yaml_value(
    node: &MarkedYaml<'_>,
    pointer: &str,
    host: &str,
    byte_offsets: &[usize],
    source_map: &mut BTreeMap<String, Mapping>,
) -> Result<Value, String> {
    let start = byte_offsets
        .get(node.span.start.index())
        .copied()
        .ok_or_else(|| format!("invalid YAML start position at {pointer}"))?;
    let end = byte_offsets
        .get(node.span.end.index())
        .copied()
        .ok_or_else(|| format!("invalid YAML end position at {pointer}"))?;
    if start > end
        || end > host.len()
        || !host.is_char_boundary(start)
        || !host.is_char_boundary(end)
    {
        return Err(format!(
            "invalid YAML node span at {pointer}: {start}..{end}"
        ));
    }
    source_map.insert(
        pointer.to_owned(),
        Mapping {
            range: Some([start as u32, end as u32]),
            text_pointer: None,
            text_map: None,
        },
    );

    if let Some(mapping) = node.data.as_mapping() {
        let mut object = serde_json::Map::new();
        for (raw_key, child) in mapping {
            let key = key_value(raw_key)?;
            let child_pointer = pointer_child(pointer, &key);
            let value = yaml_value(child, &child_pointer, host, byte_offsets, source_map)?;
            if object.insert(key.clone(), value).is_some() {
                return Err(format!("duplicate YAML mapping key {key:?} at {pointer}"));
            }
        }
        return Ok(Value::Object(object));
    }
    if let Some(sequence) = node.data.as_sequence() {
        let mut values = Vec::with_capacity(sequence.len());
        for (index, child) in sequence.iter().enumerate() {
            values.push(yaml_value(
                child,
                &pointer_child(pointer, &index.to_string()),
                host,
                byte_offsets,
                source_map,
            )?);
        }
        return Ok(Value::Array(values));
    }
    if let Some(value) = node.data.as_str() {
        if let Ok(mapped) = scalar_text(host, node, byte_offsets)
            && let Some(entry) = source_map.get_mut(pointer)
        {
            entry.text_map = text_segments(&mapped.text, &mapped.map.starts, &mapped.map.ends);
        }
        return Ok(Value::String(value.to_owned()));
    }

    let raw = host.get(start..end).unwrap_or("").trim();
    let value = if raw.is_empty() || matches!(raw.to_ascii_lowercase().as_str(), "null" | "~") {
        Value::Null
    } else if raw.eq_ignore_ascii_case("true") {
        Value::Bool(true)
    } else if raw.eq_ignore_ascii_case("false") {
        Value::Bool(false)
    } else if let Ok(value) = serde_json::from_str::<Value>(raw) {
        if value.is_number() {
            value
        } else {
            Value::String(raw.to_owned())
        }
    } else if let Some(value) = raw
        .strip_prefix("0x")
        .and_then(|value| i64::from_str_radix(value, 16).ok())
    {
        Value::from(value)
    } else {
        Value::String(raw.to_owned())
    };
    Ok(value)
}

fn key_value(key: &MarkedYaml<'_>) -> Result<String, String> {
    key.data
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "YAML selector keys must be strings".to_owned())
}

fn text_segments(text: &str, starts: &[u32], ends: &[u32]) -> Option<Vec<TextMapSegment>> {
    if text.len() != starts.len() || starts.len() != ends.len() {
        return None;
    }
    let mut segments: Vec<TextMapSegment> = Vec::new();
    for (offset, ch) in text.char_indices() {
        let text_end = offset + ch.len_utf8();
        let source_start = starts[offset];
        let source_end = ends[text_end - 1];
        if let Some(last) = segments.last_mut()
            && last.text_range[1] == offset as u32
            && last.source_range[1] == source_start
            && last.text_range[1] - last.text_range[0]
                == last.source_range[1] - last.source_range[0]
            && text_end as u32 - offset as u32 == source_end - source_start
        {
            last.text_range[1] = text_end as u32;
            last.source_range[1] = source_end;
        } else {
            segments.push(TextMapSegment {
                text_range: [offset as u32, text_end as u32],
                source_range: [source_start, source_end],
            });
        }
    }
    Some(segments)
}

fn scalar_text(
    host: &str,
    node: &MarkedYaml<'_>,
    byte_offsets: &[usize],
) -> Result<MappedScalar, String> {
    if matches!(node.data, YamlData::Alias(_) | YamlData::Tagged(_, _)) {
        return Err(
            "YAML aliases and tagged scalars have no supported direct source mapping".to_owned(),
        );
    }
    if node.data.as_mapping().is_some()
        || node.data.as_sequence().is_some()
        || node.data.is_badvalue()
    {
        return Err("selected node is not a scalar".to_owned());
    }
    let start = byte_offsets
        .get(node.span.start.index())
        .copied()
        .ok_or_else(|| "invalid YAML scalar start position".to_owned())?;
    let mut end = byte_offsets
        .get(node.span.end.index())
        .copied()
        .ok_or_else(|| "invalid YAML scalar end position".to_owned())?;
    if start > end
        || end > host.len()
        || !host.is_char_boundary(start)
        || !host.is_char_boundary(end)
    {
        return Err("YAML parser did not provide a valid byte span".to_owned());
    }
    let mut start = start;
    let mut raw = host
        .get(start..end)
        .ok_or_else(|| "invalid YAML source span".to_owned())?;
    let leading = raw.len() - raw.trim_start().len();
    if raw[leading..].starts_with(':') {
        let separator = raw[leading..]
            .find(|ch: char| !ch.is_whitespace() && ch != ':')
            .unwrap_or(raw.len() - leading);
        let prefix = leading + separator;
        start += prefix;
        end = end.saturating_add(prefix).min(host.len());
        raw = host
            .get(start..end)
            .ok_or_else(|| "invalid YAML scalar span after mapping separator".to_owned())?;
    }
    let value = node
        .data
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| raw.trim().to_owned());
    map_scalar(host, raw, start, &value)
}

fn map_scalar(host: &str, raw: &str, start: usize, value: &str) -> Result<MappedScalar, String> {
    let leading = raw.len() - raw.trim_start().len();
    let trim = raw.trim();
    let at = start + leading;
    if trim == value {
        let mut starts = Vec::new();
        let mut ends = Vec::new();
        for (offset, ch) in trim.char_indices() {
            starts.extend(std::iter::repeat_n((at + offset) as u32, ch.len_utf8()));
            ends.extend(std::iter::repeat_n(
                (at + offset + ch.len_utf8()) as u32,
                ch.len_utf8(),
            ));
        }
        return Ok(mapped_scalar(value.to_owned(), starts, ends));
    }
    if (trim.starts_with('"') && trim.ends_with('"'))
        || (trim.starts_with('\'') && trim.ends_with('\''))
    {
        return quoted_map(trim, at, value);
    }
    if trim.starts_with('|') || trim.starts_with('>') {
        return block_map(host, trim, at, value);
    }
    if value.contains('\n') || raw.contains('\n') {
        return body_map(host, raw, start, value);
    }
    Err(format!(
        "cannot map decoded scalar to exact host bytes: raw={trim:?}, value={value:?}, span={start}..{}",
        start + raw.len()
    ))
}

fn mapped_scalar(text: String, starts: Vec<u32>, ends: Vec<u32>) -> MappedScalar {
    MappedScalar {
        text,
        map: SourceOffsets { starts, ends },
    }
}

fn quoted_map(raw: &str, start: usize, value: &str) -> Result<MappedScalar, String> {
    let quote = raw.as_bytes()[0];
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut raw_at = 1;
    let raw_end = raw.len() - 1;
    let mut output = value.char_indices();
    for (out_at, ch) in output.by_ref() {
        if raw_at >= raw_end {
            return Err("quoted scalar ends before its decoded value".to_owned());
        }
        let from = raw_at;
        if quote == b'\'' && raw.as_bytes().get(raw_at..raw_at + 2) == Some(b"''") {
            raw_at += 2;
        } else if quote == b'"' && raw.as_bytes()[raw_at] == b'\\' {
            if !quoted_escape(raw, &mut raw_at, raw_end)? {
                continue;
            }
        } else if matches!(raw.as_bytes()[raw_at], b'\n' | b'\r') {
            quoted_fold(raw, &mut raw_at, raw_end, ch)?;
        } else {
            let raw_char = raw[raw_at..]
                .chars()
                .next()
                .ok_or_else(|| "invalid UTF-8 in scalar".to_owned())?;
            if raw_char != ch {
                return Err("quoted scalar source map does not match decoded value".to_owned());
            }
            raw_at += raw_char.len_utf8();
        }
        let span = (start + from) as u32..(start + raw_at) as u32;
        starts.extend(std::iter::repeat_n(span.start, ch.len_utf8()));
        ends.extend(std::iter::repeat_n(span.end, ch.len_utf8()));
        let _ = out_at;
    }
    if raw_at != raw_end {
        let remaining = &raw[raw_at..raw_end];
        if !remaining.chars().all(char::is_whitespace) {
            return Err("quoted scalar has unmapped trailing source bytes".to_owned());
        }
    }
    Ok(mapped_scalar(value.to_owned(), starts, ends))
}

fn quoted_fold(raw: &str, raw_at: &mut usize, raw_end: usize, ch: char) -> Result<(), String> {
    let newline = *raw
        .as_bytes()
        .get(*raw_at)
        .ok_or_else(|| "quoted scalar ends before its decoded value".to_owned())?;
    *raw_at += 1;
    if newline == b'\r' && raw.as_bytes().get(*raw_at) == Some(&b'\n') {
        *raw_at += 1;
    }
    while *raw_at < raw_end && matches!(raw.as_bytes()[*raw_at], b' ' | b'\t') {
        *raw_at += 1;
    }
    if !matches!(ch, ' ' | '\n') {
        return Err("quoted scalar folding does not match decoded value".to_owned());
    }
    Ok(())
}

fn quoted_escape(raw: &str, raw_at: &mut usize, raw_end: usize) -> Result<bool, String> {
    *raw_at += 1;
    let escape = *raw
        .as_bytes()
        .get(*raw_at)
        .ok_or_else(|| "unfinished YAML escape".to_owned())?;
    *raw_at += 1;
    let length = match escape {
        b'0' | b'a' | b'b' | b't' | b'\t' | b'n' | b'v' | b'f' | b'r' | b'e' | b' ' | b'"'
        | b'/' | b'\\' | b'N' | b'_' | b'L' | b'P' => 0,
        b'x' => 2,
        b'u' => 4,
        b'U' => 8,
        b'\n' => 0,
        b'\r' => {
            if raw.as_bytes().get(*raw_at) == Some(&b'\n') {
                *raw_at += 1;
            }
            0
        }
        _ => return Err("unknown YAML escape sequence".to_owned()),
    };
    if matches!(escape, b'\n' | b'\r') {
        while *raw_at < raw_end && raw.as_bytes()[*raw_at].is_ascii_whitespace() {
            *raw_at += 1;
        }
        return Ok(false);
    }
    *raw_at += length;
    Ok(true)
}

fn body_map(host: &str, raw: &str, start: usize, value: &str) -> Result<MappedScalar, String> {
    let mut cursor = 0;
    let mut starts = Vec::with_capacity(value.len());
    let mut ends = Vec::with_capacity(value.len());
    for ch in value.chars() {
        let found = if ch.is_whitespace() {
            let exact = raw[cursor..]
                .char_indices()
                .find(|(_, candidate)| *candidate == ch);
            exact.or_else(|| {
                raw[cursor..]
                    .char_indices()
                    .find(|(_, candidate)| candidate.is_whitespace())
            })
        } else {
            raw[cursor..]
                .char_indices()
                .find(|(_, candidate)| *candidate == ch)
        };
        let Some((relative, source_char)) = found else {
            return Err(format!("YAML decoded scalar cannot be mapped at {ch:?}"));
        };
        let from = cursor + relative;
        let to = from + source_char.len_utf8();
        starts.extend(std::iter::repeat_n((start + from) as u32, ch.len_utf8()));
        ends.extend(std::iter::repeat_n((start + to) as u32, ch.len_utf8()));
        cursor = to;
    }
    if value.is_empty() {
        return Ok(mapped_scalar(String::new(), starts, ends));
    }
    let (_, source_end) = map_bounds(&starts, &ends, start as u32);
    if source_end as usize > host.len() {
        return Err("YAML source map exceeds host text".to_owned());
    }
    Ok(mapped_scalar(value.to_owned(), starts, ends))
}

fn block_map(host: &str, raw: &str, start: usize, value: &str) -> Result<MappedScalar, String> {
    let newline = raw
        .find('\n')
        .ok_or_else(|| "block scalar has no body".to_owned())?;
    let header = &raw[..newline];
    let style = header
        .chars()
        .next()
        .ok_or_else(|| "empty block scalar header".to_owned())?;
    let explicit_indent = header
        .chars()
        .find(char::is_ascii_digit)
        .and_then(|ch| ch.to_digit(10))
        .map(|n| n as usize);
    let header_indent = host[..start]
        .rsplit_once('\n')
        .map_or(host.len() - host.trim_start().len(), |(_, line)| {
            line.bytes().take_while(|byte| *byte == b' ').count()
        });
    let body_at = start + newline + 1;
    let body = &host[body_at..start + raw.len()];
    let lines: Vec<(usize, &str)> = body
        .split_inclusive('\n')
        .scan(body_at, |at, line| {
            let current = *at;
            *at += line.len();
            Some((current, line.trim_end_matches(['\n', '\r'])))
        })
        .collect();
    let indent = explicit_indent
        .map(|n| header_indent + n)
        .unwrap_or_else(|| {
            lines
                .iter()
                .filter(|(_, line)| !line.trim().is_empty())
                .map(|(_, line)| line.bytes().take_while(|byte| *byte == b' ').count())
                .min()
                .unwrap_or(header_indent + 1)
        });
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut text = String::new();
    for (index, (line_start, line)) in lines.iter().enumerate() {
        let leading = line
            .bytes()
            .take_while(|byte| *byte == b' ')
            .count()
            .min(indent);
        let content = &line[leading..];
        text.push_str(content);
        for (offset, ch) in content.char_indices() {
            starts.extend(std::iter::repeat_n(
                (line_start + leading + offset) as u32,
                ch.len_utf8(),
            ));
            ends.extend(std::iter::repeat_n(
                (line_start + leading + offset + ch.len_utf8()) as u32,
                ch.len_utf8(),
            ));
        }
        let has_newline = body
            .get((line_start + line.len()).saturating_sub(body_at)..)
            .is_some_and(|tail| tail.starts_with('\n') || tail.starts_with("\r\n"));
        if has_newline && index + 1 < lines.len() {
            let next = lines[index + 1].1;
            let separator = if style == '>' && !content.is_empty() && !next.trim().is_empty() {
                ' '
            } else {
                '\n'
            };
            let newline_at = line_start + line.len();
            text.push(separator);
            starts.extend(std::iter::repeat_n(newline_at as u32, separator.len_utf8()));
            ends.extend(std::iter::repeat_n(
                (newline_at + 1) as u32,
                separator.len_utf8(),
            ));
        }
    }
    while text.len() > value.len() || !value.starts_with(&text) {
        if text.ends_with('\n') && !value.ends_with('\n') {
            text.pop();
            starts.pop();
            ends.pop();
        } else {
            return Err("block scalar source map does not match YAML-decoded value".to_owned());
        }
    }
    if text != value {
        return Err("block scalar source map is incomplete".to_owned());
    }
    Ok(mapped_scalar(text, starts, ends))
}

fn map_bounds(starts: &[u32], ends: &[u32], fallback: u32) -> (u32, u32) {
    (
        starts.first().copied().unwrap_or(fallback),
        ends.last().copied().unwrap_or(fallback),
    )
}

fn main() {
    let mut provider = Provider {
        version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let input = std::io::BufReader::new(std::io::stdin().lock());
    if let Err(error) = lighthouse_protocol::serve(input, std::io::stdout().lock(), &mut provider) {
        eprintln!("{PLUGIN}: {error}");
    }
}

#[cfg(test)]
mod tests;
