use std::{collections::BTreeMap, fs, ops::Range, path::PathBuf};

use lighthouse_protocol::{
    Conventions, ErrorObject, FileInfo, Fragment, Handler, Incomplete, IndexInput, IndexParams,
    IndexResult, InitializeParams, InitializeResult, Mapping, ProviderManifest, Source,
    TextMapSegment, pointer_child,
};
use serde_json::Value;
use toml_edit::{Document, Item, TableLike, Value as TomlValue};

const LANGUAGE: &str = "toml";
const PLUGIN: &str = "lang-toml";

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
                globs: vec!["**/*.toml".to_owned()],
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
                            "{} provider does not analyze selected source regions",
                            LANGUAGE
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
                            reason: format!("cannot read TOML host document: {error}"),
                        });
                        continue;
                    }
                };
                &owned
            };
            match Document::parse(text) {
                Ok(document) => {
                    let mut parsed = vec![Node {
                        path: Vec::new(),
                        kind: Kind::Object,
                        span: Some(0..text.len()),
                    }];
                    if let Err(reason) =
                        collect_table(document.as_table(), &mut Vec::new(), &mut parsed)
                    {
                        result.incomplete.push(Incomplete {
                            path: Some(file.path.clone()),
                            reason: format!("cannot expose TOML syntax tree: {reason}"),
                        });
                        continue;
                    }
                    match source_ast(&parsed, text, &file.path) {
                        Ok(ast) => {
                            result.sources.push(ast);
                            result.fragments.push(Fragment {
                                file: FileInfo {
                                    path: file.path.clone(),
                                    ..FileInfo::default()
                                },
                                ..Fragment::default()
                            });
                        }
                        Err(reason) => result.incomplete.push(Incomplete {
                            path: Some(file.path.clone()),
                            reason: format!("cannot map TOML AST: {reason}"),
                        }),
                    }
                }
                Err(error) => result.incomplete.push(Incomplete {
                    path: Some(file.path.clone()),
                    reason: format!("invalid TOML: {error}"),
                }),
            }
        }
        Ok(result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone)]
enum Kind {
    Object,
    Array,
    String(String),
    Number(Value),
    Boolean(bool),
    Other(Value),
}

#[derive(Debug, Clone)]
struct Node {
    path: Vec<Segment>,
    kind: Kind,
    span: Option<Range<usize>>,
}

#[derive(Clone)]
struct SourceOffsets {
    starts: Vec<u32>,
    ends: Vec<u32>,
}

struct StringMapping<'a> {
    raw: &'a str,
    base: usize,
    end: usize,
    multiline: bool,
    value: &'a str,
    out: usize,
    starts: Vec<u32>,
    ends: Vec<u32>,
}

impl StringMapping<'_> {
    fn push(&mut self, from: usize, to: usize, ch: char) {
        self.starts.extend(std::iter::repeat_n(
            (self.base + from) as u32,
            ch.len_utf8(),
        ));
        self.ends
            .extend(std::iter::repeat_n((self.base + to) as u32, ch.len_utf8()));
        self.out += ch.len_utf8();
    }

    fn escape(&mut self, at: &mut usize) -> Result<(), String> {
        let bytes = self.raw.as_bytes();
        let next = *bytes
            .get(*at + 1)
            .ok_or_else(|| "unfinished TOML escape".to_owned())?;
        if self.multiline && matches!(next, b'\n' | b'\r') {
            *at += if next == b'\r' && bytes.get(*at + 2) == Some(&b'\n') {
                3
            } else {
                2
            };
            while *at < self.end && bytes[*at].is_ascii_whitespace() {
                *at += 1;
            }
            return Ok(());
        }
        let consumed = match next {
            b'u' => 6,
            b'U' => 10,
            _ => 2,
        };
        if *at + consumed > self.end {
            return Err("unfinished TOML escape".to_owned());
        }
        let ch = self
            .value
            .get(self.out..)
            .and_then(|tail| tail.chars().next())
            .ok_or_else(|| "TOML escape has no decoded character".to_owned())?;
        let from = *at;
        *at += consumed;
        self.push(from, *at, ch);
        Ok(())
    }
}

fn collect_table(
    table: &dyn TableLike,
    path: &mut Vec<Segment>,
    nodes: &mut Vec<Node>,
) -> Result<(), String> {
    for (key, item) in table.iter() {
        path.push(Segment::Key(key.to_owned()));
        collect_item(item, path, nodes)?;
        path.pop();
    }
    Ok(())
}

fn collect_item(item: &Item, path: &mut Vec<Segment>, nodes: &mut Vec<Node>) -> Result<(), String> {
    if let Some(table) = item.as_table_like() {
        nodes.push(Node {
            path: path.clone(),
            kind: Kind::Object,
            span: item.span(),
        });
        collect_table(table, path, nodes)?;
    } else if let Some(tables) = item.as_array_of_tables() {
        nodes.push(Node {
            path: path.clone(),
            kind: Kind::Array,
            span: item.span(),
        });
        for (index, table) in tables.iter().enumerate() {
            path.push(Segment::Index(index));
            nodes.push(Node {
                path: path.clone(),
                kind: Kind::Object,
                span: table.span(),
            });
            collect_table(table, path, nodes)?;
            path.pop();
        }
    } else if let Some(array) = item.as_array() {
        nodes.push(Node {
            path: path.clone(),
            kind: Kind::Array,
            span: array.span(),
        });
        for (index, value) in array.iter().enumerate() {
            path.push(Segment::Index(index));
            collect_value(value, path, nodes)?;
            path.pop();
        }
    } else if let Some(value) = item.as_value() {
        collect_value(value, path, nodes)?;
    } else {
        nodes.push(Node {
            path: path.clone(),
            kind: Kind::Other(Value::Null),
            span: item.span(),
        });
    }
    Ok(())
}

fn collect_value(
    value: &TomlValue,
    path: &mut Vec<Segment>,
    nodes: &mut Vec<Node>,
) -> Result<(), String> {
    let span = value.span();
    if let Some(table) = value.as_inline_table() {
        nodes.push(Node {
            path: path.clone(),
            kind: Kind::Object,
            span,
        });
        for (key, item) in table.iter() {
            path.push(Segment::Key(key.to_owned()));
            collect_value(item, path, nodes)?;
            path.pop();
        }
    } else if let Some(array) = value.as_array() {
        nodes.push(Node {
            path: path.clone(),
            kind: Kind::Array,
            span,
        });
        for (index, item) in array.iter().enumerate() {
            path.push(Segment::Index(index));
            collect_value(item, path, nodes)?;
            path.pop();
        }
    } else if let Some(text) = value.as_str() {
        nodes.push(Node {
            path: path.clone(),
            kind: Kind::String(text.to_owned()),
            span,
        });
    } else if let Some(value) = value.as_bool() {
        nodes.push(Node {
            path: path.clone(),
            kind: Kind::Boolean(value),
            span,
        });
    } else if let Some(value) = value.as_integer() {
        nodes.push(Node {
            path: path.clone(),
            kind: Kind::Number(Value::from(value)),
            span,
        });
    } else if let Some(value) = value.as_float() {
        let number = serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null);
        nodes.push(Node {
            path: path.clone(),
            kind: Kind::Number(number),
            span,
        });
    } else {
        nodes.push(Node {
            path: path.clone(),
            kind: Kind::Other(Value::String(value.to_string())),
            span,
        });
    }
    Ok(())
}

fn source_ast(nodes: &[Node], host: &str, path: &str) -> Result<Source, String> {
    let mut ast = Value::Object(serde_json::Map::new());
    let mut source_map = BTreeMap::new();
    for node in nodes {
        let pointer = node
            .path
            .iter()
            .fold(String::new(), |pointer, segment| match segment {
                Segment::Key(key) => pointer_child(&pointer, key),
                Segment::Index(index) => pointer_child(&pointer, &index.to_string()),
            });
        let value = match &node.kind {
            Kind::Object => Value::Object(serde_json::Map::new()),
            Kind::Array => Value::Array(Vec::new()),
            Kind::String(text) => Value::String(text.clone()),
            Kind::Number(value) | Kind::Other(value) => value.clone(),
            Kind::Boolean(value) => Value::Bool(*value),
        };
        if let Some(span) = &node.span {
            if span.start > span.end
                || span.end > host.len()
                || !host.is_char_boundary(span.start)
                || !host.is_char_boundary(span.end)
            {
                return Err(format!(
                    "invalid TOML source range at {pointer}: {}..{}",
                    span.start, span.end
                ));
            }
            let mut entry = Mapping {
                range: Some([span.start as u32, span.end as u32]),
                text_pointer: None,
                text_map: None,
            };
            if let Kind::String(text) = &node.kind
                && let Ok(mapped) = map_string(host, span.clone(), text)
            {
                entry.text_map = text_segments(text, &mapped.starts, &mapped.ends);
            }
            source_map.insert(pointer.clone(), entry);
        }
        if node.path.is_empty() {
            ast = value;
        } else {
            insert_path(&mut ast, &node.path, value)?;
        }
    }
    Ok(Source {
        path: path.to_owned(),
        ast,
        mapping: source_map,
    })
}

fn insert_path(root: &mut Value, path: &[Segment], value: Value) -> Result<(), String> {
    let (last, parents) = path
        .split_last()
        .ok_or_else(|| "empty TOML insertion path".to_owned())?;
    let mut parent = root;
    for segment in parents {
        parent = match (segment, parent) {
            (Segment::Key(key), Value::Object(object)) => object
                .get_mut(key)
                .ok_or_else(|| format!("missing TOML AST parent field {key}"))?,
            (Segment::Index(index), Value::Array(array)) => array
                .get_mut(*index)
                .ok_or_else(|| format!("missing TOML AST parent array index {index}"))?,
            _ => return Err("TOML AST parent shape disagrees with parser path".to_owned()),
        };
    }
    match (last, parent) {
        (Segment::Key(key), Value::Object(object)) => {
            object.insert(key.clone(), value);
        }
        (Segment::Index(index), Value::Array(array)) => {
            if *index >= array.len() {
                array.resize(index + 1, Value::Null);
            }
            array[*index] = value;
        }
        _ => return Err("TOML AST parent shape disagrees with parser path".to_owned()),
    }
    Ok(())
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

fn map_string(host: &str, range: Range<usize>, value: &str) -> Result<SourceOffsets, String> {
    let raw = host
        .get(range.clone())
        .ok_or_else(|| "TOML source span is outside host text".to_owned())?;
    let bytes = raw.as_bytes();
    let quote = *bytes
        .first()
        .ok_or_else(|| "empty TOML string token".to_owned())?;
    if !matches!(quote, b'\'' | b'"') {
        return Err("selected TOML string has no quote delimiter".to_owned());
    }
    let multiline = bytes.get(1..3) == Some(&[quote, quote]);
    let delimiter = if multiline { 3 } else { 1 };
    if bytes.len() < delimiter * 2 {
        return Err("invalid TOML string delimiter span".to_owned());
    }
    let end = bytes.len() - delimiter;
    let mut at = delimiter;
    if multiline {
        if raw[at..].starts_with("\r\n") {
            at += 2;
        } else if raw[at..].starts_with('\n') || raw[at..].starts_with('\r') {
            at += 1;
        }
    }
    let mut mapping = StringMapping {
        raw,
        base: range.start,
        end,
        multiline,
        value,
        out: 0,
        starts: Vec::with_capacity(value.len()),
        ends: Vec::with_capacity(value.len()),
    };
    while at < end {
        if quote == b'"' && bytes[at] == b'\\' {
            mapping.escape(&mut at)?;
            continue;
        }
        let from = at;
        let ch = raw[at..]
            .chars()
            .next()
            .ok_or_else(|| "invalid UTF-8 in TOML string".to_owned())?;
        if ch == '\r' && raw[at..].starts_with("\r\n") {
            at += 2;
        } else {
            at += ch.len_utf8();
        }
        let decoded = if ch == '\r' { '\n' } else { ch };
        if value
            .get(mapping.out..)
            .and_then(|tail| tail.chars().next())
            != Some(decoded)
        {
            return Err(format!(
                "TOML decoded value differs at source byte {}: expected {:?}",
                range.start + from,
                value.get(mapping.out..)
            ));
        }
        mapping.push(from, at, decoded);
    }
    if mapping.out != value.len() {
        return Err(format!(
            "TOML source map covered {} decoded bytes, expected {}",
            mapping.out,
            value.len()
        ));
    }
    Ok(SourceOffsets {
        starts: mapping.starts,
        ends: mapping.ends,
    })
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
