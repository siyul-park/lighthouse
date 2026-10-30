//! The tables, arrays and values of a TOML document as JSON, and where each
//! lies.

use std::{collections::BTreeMap, ops::Range};

use lighthouse_protocol::{
    Mapping, TextMapSegment,
    language::{HostError, HostLanguage, HostParser, HostTree},
    pointer_child,
};
use serde_json::{Map, Number, Value};
use toml_edit::{Document, Item, TableLike, Value as TomlValue};

use crate::{comments, regions};

pub(crate) struct Toml;

impl HostParser for Toml {
    fn language(&self) -> HostLanguage {
        HostLanguage {
            name: "lang-toml",
            version: env!("CARGO_PKG_VERSION"),
            id: "toml",
            title: "TOML",
            aliases: &["toml"],
            globs: &["**/*.toml"],
        }
    }

    fn parse(&self, text: &str) -> Result<HostTree, HostError> {
        parse(text)
    }
}

struct Walk<'h> {
    host: &'h str,
    mapping: BTreeMap<String, Mapping>,
}

impl Walk<'_> {
    fn table(&mut self, table: &dyn TableLike, pointer: &str) -> Result<Value, String> {
        let mut object = Map::new();
        for (key, item) in table.iter() {
            object.insert(
                key.to_owned(),
                self.item(item, &pointer_child(pointer, key))?,
            );
        }
        Ok(Value::Object(object))
    }

    fn item(&mut self, item: &Item, pointer: &str) -> Result<Value, String> {
        if let Some(table) = item.as_table_like() {
            self.record(pointer.to_owned(), item.span(), None)?;
            return self.table(table, pointer);
        }
        if let Some(tables) = item.as_array_of_tables() {
            self.record(pointer.to_owned(), item.span(), None)?;
            let mut values = Vec::with_capacity(tables.len());
            for (index, table) in tables.iter().enumerate() {
                let pointer = pointer_child(pointer, &index.to_string());
                self.record(pointer.clone(), table.span(), None)?;
                values.push(self.table(table, &pointer)?);
            }
            return Ok(Value::Array(values));
        }
        if let Some(value) = item.as_value() {
            return self.value(value, pointer);
        }
        self.record(pointer.to_owned(), item.span(), None)?;
        Ok(Value::Null)
    }

    fn value(&mut self, value: &TomlValue, pointer: &str) -> Result<Value, String> {
        let span = value.span();
        if let Some(table) = value.as_inline_table() {
            self.record(pointer.to_owned(), span, None)?;
            let mut object = Map::new();
            for (key, item) in table.iter() {
                object.insert(
                    key.to_owned(),
                    self.value(item, &pointer_child(pointer, key))?,
                );
            }
            return Ok(Value::Object(object));
        }
        if let Some(array) = value.as_array() {
            self.record(pointer.to_owned(), span, None)?;
            let mut values = Vec::with_capacity(array.len());
            for (index, item) in array.iter().enumerate() {
                values.push(self.value(item, &pointer_child(pointer, &index.to_string()))?);
            }
            return Ok(Value::Array(values));
        }
        if let Some(text) = value.as_str() {
            let text_map = match &span {
                Some(span) => {
                    let offsets = regions::map(self.host, span.clone(), text)?;
                    Some(
                        offsets
                            .text_map(text)
                            .ok_or_else(|| format!("invalid TOML text map at {pointer}"))?,
                    )
                }
                None => None,
            };
            self.record(pointer.to_owned(), span, text_map)?;
            return Ok(Value::String(text.to_owned()));
        }
        let plain = if let Some(flag) = value.as_bool() {
            Value::Bool(flag)
        } else if let Some(number) = value.as_integer() {
            Value::from(number)
        } else if let Some(number) = value.as_float().and_then(Number::from_f64) {
            Value::Number(number)
        } else {
            // A datetime, or a float JSON cannot hold (`inf`, `nan`): the text.
            Value::String(self.raw(value, span.as_ref()))
        };
        self.record(pointer.to_owned(), span, None)?;
        Ok(plain)
    }

    /// Records where the node at `pointer` lies, when the parser says.
    fn record(
        &mut self,
        pointer: String,
        span: Option<Range<usize>>,
        text_map: Option<Vec<TextMapSegment>>,
    ) -> Result<(), String> {
        let Some(span) = span else {
            return Ok(());
        };
        if span.start > span.end
            || span.end > self.host.len()
            || !self.host.is_char_boundary(span.start)
            || !self.host.is_char_boundary(span.end)
        {
            return Err(format!(
                "invalid TOML source range at {pointer}: {}..{}",
                span.start, span.end
            ));
        }
        self.mapping.insert(
            pointer,
            Mapping {
                range: Some([span.start as u32, span.end as u32]),
                text_pointer: None,
                text_map,
            },
        );
        Ok(())
    }

    /// The source text of a value, without the white space around it.
    fn raw(&self, value: &TomlValue, span: Option<&Range<usize>>) -> String {
        span.and_then(|span| self.host.get(span.clone()))
            .map_or_else(|| value.to_string(), str::to_owned)
            .trim()
            .to_owned()
    }
}

/// The tree of `text`: the root table.
pub(crate) fn parse(text: &str) -> Result<HostTree, HostError> {
    let document = Document::parse(text).map_err(|error| HostError::Syntax(error.to_string()))?;
    let mut walk = Walk {
        host: text,
        mapping: BTreeMap::new(),
    };
    walk.record(String::new(), Some(0..text.len()), None)
        .map_err(HostError::Tree)?;
    let ast = walk
        .table(document.as_table(), "")
        .map_err(HostError::Tree)?;
    Ok(HostTree {
        ast,
        mapping: walk.mapping,
        comments: comments::of(text, &document),
    })
}
