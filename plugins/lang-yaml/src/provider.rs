//! The documents of a YAML stream as JSON, and where each value lies.

use std::collections::BTreeMap;

use lighthouse_protocol::{
    Mapping,
    language::{HostError, HostLanguage, HostParser, HostTree},
    pointer_child,
};
use saphyr::{LoadableYamlNode, MarkedYaml, Scalar, YamlData};
use serde_json::{Number, Value, json};

use crate::regions::{self, MappedScalar};

/// A walk over the nodes of a document, which records where each lies.
pub(crate) struct Yaml;

impl HostParser for Yaml {
    fn language(&self) -> HostLanguage {
        HostLanguage {
            name: "lang-yaml",
            version: env!("CARGO_PKG_VERSION"),
            id: "yaml",
            title: "YAML",
            aliases: &["yaml", "yml"],
            globs: &["**/*.yaml", "**/*.yml"],
        }
    }

    fn parse(&self, text: &str) -> Result<HostTree, HostError> {
        parse(text)
    }
}

pub(crate) struct Walk<'h> {
    host: &'h str,
    /// The byte offset of each character index the parser reports.
    byte_offsets: Vec<usize>,
    pub(crate) mapping: BTreeMap<String, Mapping>,
}

impl<'h> Walk<'h> {
    pub(crate) fn new(host: &'h str) -> Self {
        let byte_offsets = host
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(host.len()))
            .collect();
        Self {
            host,
            byte_offsets,
            mapping: BTreeMap::new(),
        }
    }

    /// The JSON value of `node` at `pointer`, with its place in the mapping.
    pub(crate) fn value(&mut self, node: &MarkedYaml<'_>, pointer: &str) -> Result<Value, String> {
        let (start, end) = self
            .range(node)
            .map_err(|reason| format!("{reason} at {pointer}"))?;
        let mut entry = range_entry(start, end);
        let value = match &node.data {
            YamlData::Mapping(mapping) => {
                let mut object = serde_json::Map::new();
                for (raw_key, child) in mapping {
                    let key = key_value(raw_key)?;
                    let value = self.value(child, &pointer_child(pointer, &key))?;
                    object.insert(key, value);
                }
                Value::Object(object)
            }
            YamlData::Sequence(sequence) => {
                let mut values = Vec::with_capacity(sequence.len());
                for (index, child) in sequence.iter().enumerate() {
                    values.push(self.value(child, &pointer_child(pointer, &index.to_string()))?);
                }
                Value::Array(values)
            }
            YamlData::Value(Scalar::String(text)) => {
                let mapped = self.text(node)?;
                entry.text_map = Some(
                    mapped
                        .offsets
                        .text_map(&mapped.text)
                        .ok_or_else(|| format!("invalid YAML text map at {pointer}"))?,
                );
                Value::String(text.to_string())
            }
            YamlData::Value(scalar) => self.plain(scalar, start, end, pointer)?,
            _ => self.raw(start, end, pointer)?,
        };
        self.mapping.insert(pointer.to_owned(), entry);
        Ok(value)
    }

    /// The decoded text of the string scalar `node`, with where each byte came
    /// from. A scalar after a mapping separator starts past it.
    fn text(&self, node: &MarkedYaml<'_>) -> Result<MappedScalar, String> {
        let value = node
            .data
            .as_str()
            .ok_or_else(|| "selected node is not a string scalar".to_owned())?;
        let (mut start, mut end) = self.range(node)?;
        let mut raw = self
            .host
            .get(start..end)
            .ok_or_else(|| "invalid YAML source span".to_owned())?;
        let leading = raw.len() - raw.trim_start().len();
        if raw[leading..].starts_with(':') {
            let separator = raw[leading..]
                .find(|ch: char| !ch.is_whitespace() && ch != ':')
                .unwrap_or(raw.len() - leading);
            let prefix = leading + separator;
            start += prefix;
            end = end.saturating_add(prefix).min(self.host.len());
            raw = self
                .host
                .get(start..end)
                .ok_or_else(|| "invalid YAML scalar span after mapping separator".to_owned())?;
        }
        regions::map_scalar(self.host, raw, start, value)
    }

    /// A scalar the parser resolved, as the core schema of YAML 1.2 has it. A
    /// float that JSON cannot hold (`.inf`, `.nan`) stays the text it is.
    fn plain(
        &self,
        scalar: &Scalar<'_>,
        start: usize,
        end: usize,
        pointer: &str,
    ) -> Result<Value, String> {
        Ok(match scalar {
            Scalar::Null => Value::Null,
            Scalar::Boolean(flag) => Value::Bool(*flag),
            Scalar::Integer(number) => Value::from(*number),
            Scalar::FloatingPoint(number) => match Number::from_f64(number.into_inner()) {
                Some(number) => Value::Number(number),
                None => self.raw(start, end, pointer)?,
            },
            Scalar::String(text) => Value::String(text.to_string()),
        })
    }

    /// The source text of a node with no resolved value, as a string.
    fn raw(&self, start: usize, end: usize, pointer: &str) -> Result<Value, String> {
        let text = self
            .host
            .get(start..end)
            .ok_or_else(|| format!("invalid YAML scalar range at {pointer}"))?;
        Ok(Value::String(text.trim().to_owned()))
    }

    /// The byte range of `node` in the host text.
    pub(crate) fn range(&self, node: &MarkedYaml<'_>) -> Result<(usize, usize), String> {
        let at = |index: usize| {
            self.byte_offsets
                .get(index)
                .copied()
                .ok_or_else(|| format!("invalid YAML position {index}"))
        };
        let (start, end) = (at(node.span.start.index())?, at(node.span.end.index())?);
        if start > end || !self.host.is_char_boundary(start) || !self.host.is_char_boundary(end) {
            return Err(format!("invalid YAML node span {start}..{end}"));
        }
        Ok((start, end))
    }
}

fn key_value(key: &MarkedYaml<'_>) -> Result<String, String> {
    key.data
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "YAML selector keys must be strings".to_owned())
}

/// The tree of `text`: `{"kind": "Stream", "documents": [...]}`.
pub(crate) fn parse(text: &str) -> Result<HostTree, HostError> {
    let documents =
        MarkedYaml::load_from_str(text).map_err(|error| HostError::Syntax(error.to_string()))?;
    let whole = range_entry(0, text.len());
    let mut walk = Walk::new(text);
    walk.mapping.insert(String::new(), whole.clone());
    walk.mapping.insert("/documents".to_owned(), whole);
    let mut values = Vec::with_capacity(documents.len());
    for (index, root) in documents.iter().enumerate() {
        let pointer = pointer_child("/documents", &index.to_string());
        values.push(walk.value(root, &pointer).map_err(HostError::Tree)?);
    }
    Ok(HostTree {
        ast: json!({"kind": "Stream", "documents": values}),
        mapping: walk.mapping,
        comments: Vec::new(),
    })
}

fn range_entry(start: usize, end: usize) -> Mapping {
    Mapping {
        range: Some([start as u32, end as u32]),
        text_pointer: None,
        text_map: None,
    }
}
