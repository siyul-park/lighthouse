//! The comments of a TOML document, which the parser keeps in the decorations
//! of its keys and values.

use std::{collections::BTreeSet, ops::Range};

use lighthouse_protocol::{Comment, Position, Span};
use toml_edit::{Decor, Document, Item, RawString, TableLike, Value as TomlValue};

/// The comments TOML's parser retains in the decorations of keys and values.
pub(crate) fn of(text: &str, document: &Document<&str>) -> Vec<Comment> {
    let table = document.as_table();
    let mut spans = BTreeSet::<(usize, usize)>::new();
    decorations(table.decor(), text, &mut spans);
    table_comments(table, text, &mut spans);
    raw_comments(document.trailing(), text, &mut spans);
    let mut grouped: Vec<Range<usize>> = Vec::new();
    for (start, end) in spans {
        let span = start..end;
        if let Some(last) = grouped.last_mut() {
            let gap = &text[last.end..span.start];
            if gap == "\n" || gap == "\r\n" {
                last.end = span.end;
                continue;
            }
        }
        grouped.push(span);
    }
    grouped
        .into_iter()
        .map(|span| Comment {
            span: Span {
                start: position(text, span.start),
                end: position(text, span.end),
            },
            text: text[span].to_owned(),
            attached_to: None,
        })
        .collect()
}

fn table_comments(table: &dyn TableLike, text: &str, spans: &mut BTreeSet<(usize, usize)>) {
    for (name, item) in table.iter() {
        if let Some(key) = table.key(name) {
            decorations(key.leaf_decor(), text, spans);
            decorations(key.dotted_decor(), text, spans);
        }
        item_comments(item, text, spans);
    }
}

fn item_comments(item: &Item, text: &str, spans: &mut BTreeSet<(usize, usize)>) {
    if let Some(table) = item.as_table() {
        decorations(table.decor(), text, spans);
        table_comments(table, text, spans);
    } else if let Some(tables) = item.as_array_of_tables() {
        for table in tables.iter() {
            decorations(table.decor(), text, spans);
            table_comments(table, text, spans);
        }
    } else if let Some(value) = item.as_value() {
        value_comments(value, text, spans);
    }
}

fn value_comments(value: &TomlValue, text: &str, spans: &mut BTreeSet<(usize, usize)>) {
    decorations(value.decor(), text, spans);
    if let Some(array) = value.as_array() {
        for value in array.iter() {
            value_comments(value, text, spans);
        }
        raw_comments(array.trailing(), text, spans);
    } else if let Some(table) = value.as_inline_table() {
        decorations(table.decor(), text, spans);
        table_comments(table, text, spans);
        raw_comments(table.trailing(), text, spans);
    }
}

/// Finds comment markers only inside parser-owned decoration spans, not in values.
fn decorations(decor: &Decor, text: &str, spans: &mut BTreeSet<(usize, usize)>) {
    for raw in [decor.prefix(), decor.suffix()].into_iter().flatten() {
        raw_comments(raw, text, spans);
    }
}

/// The comments in one span of text the parser kept apart from the values.
fn raw_comments(raw: &RawString, text: &str, spans: &mut BTreeSet<(usize, usize)>) {
    let Some(range) = raw.span() else { return };
    let Some(source) = text.get(range.clone()) else {
        return;
    };
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        if let Some(marker) = line.find('#') {
            let content = line[marker..].trim_end_matches(['\n', '\r']);
            if !content.is_empty() {
                let start = range.start + offset + marker;
                spans.insert((start, start + content.len()));
            }
        }
        offset += line.len();
    }
}

fn position(text: &str, offset: usize) -> Position {
    let before = &text[..offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let column = before
        .rfind('\n')
        .map_or(before.len(), |start| before.len() - start - 1) as u32
        + 1;
    Position { line, col: column }
}
