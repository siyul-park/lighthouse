//! A YAML writer for documents people read and review: long sentences fold
//! across lines, multi-line text is a literal block, short lists of words stay
//! on one line. Everything it writes reads back as the same value.

use std::fmt::Write;

use serde::Serialize;
use serde_norway::{Mapping, Value};

const WIDTH: usize = 80;
const INDENT: usize = 2;

/// `value` as YAML. Keys keep the order the value serializes them in: the
/// order of a struct's fields, the sorted order of a map.
pub fn to_yaml<T: Serialize>(value: &T) -> String {
    let value = serde_norway::to_value(value).expect("a document serializes to YAML");
    let mut out = String::new();
    match &value {
        Value::Mapping(map) if !map.is_empty() => block_map(&mut out, map, 0),
        Value::Sequence(items) if !items.is_empty() => block_list(&mut out, items, 0),
        other => {
            out.push_str(&scalar_or_empty(other));
            out.push('\n');
        }
    }
    out
}

fn block_map(out: &mut String, map: &Mapping, indent: usize) {
    for (key, value) in map {
        let _ = write!(out, "{}{}:", " ".repeat(indent), key_text(key));
        entry(out, value, indent);
    }
}

fn block_list(out: &mut String, items: &[Value], indent: usize) {
    for item in items {
        let pad = " ".repeat(indent);
        match item {
            Value::Mapping(map) if !map.is_empty() => {
                let mut inner = String::new();
                block_map(&mut inner, map, indent + INDENT);
                // The first key shares the dash line.
                let rest = &inner[indent + INDENT..];
                let _ = write!(out, "{pad}- {rest}");
            }
            Value::Sequence(nested) if !nested.is_empty() && !flow_fits(nested, indent + 2) => {
                let _ = writeln!(out, "{pad}-");
                block_list(out, nested, indent + INDENT);
            }
            other => {
                let _ = write!(out, "{pad}- ");
                inline_value(out, other, indent + INDENT);
            }
        }
    }
}

/// What follows `key:` for `value`, ending the line.
fn entry(out: &mut String, value: &Value, indent: usize) {
    match value {
        Value::Mapping(map) if !map.is_empty() => {
            out.push('\n');
            block_map(out, map, indent + INDENT);
        }
        Value::Sequence(items) if !items.is_empty() => {
            if flow_fits(items, indent + 2) {
                let _ = writeln!(out, " {}", flow(items));
            } else {
                out.push('\n');
                block_list(out, items, indent + INDENT);
            }
        }
        other => {
            out.push(' ');
            inline_value(out, other, indent + INDENT);
        }
    }
}

/// A scalar, a short flow list, or a block string, ending the line.
fn inline_value(out: &mut String, value: &Value, indent: usize) {
    match value {
        Value::String(text) => string(out, text, indent),
        Value::Sequence(items) if !items.is_empty() => {
            if flow_fits(items, indent) {
                let _ = writeln!(out, "{}", flow(items));
            } else {
                out.push('\n');
                block_list(out, items, indent);
            }
        }
        other => {
            out.push_str(&scalar_or_empty(other));
            out.push('\n');
        }
    }
}

fn string(out: &mut String, text: &str, indent: usize) {
    if text.contains('\n') {
        if let Some(block) = literal(text, indent) {
            out.push_str(&block);
            return;
        }
    } else if text.chars().count() + indent > WIDTH
        && let Some(block) = folded(text, indent)
    {
        out.push_str(&block);
        return;
    }
    out.push_str(&scalar_text(text));
    out.push('\n');
}

fn literal(text: &str, indent: usize) -> Option<String> {
    if text.starts_with([' ', '\n'])
        || text
            .chars()
            .any(|c| c == '\r' || c == '\t' || c.is_control() && c != '\n')
    {
        return None;
    }
    let (body, header) = match text.strip_suffix('\n') {
        Some(body) if !body.ends_with('\n') => (body, "|"),
        Some(_) => return None,
        None => (text, "|-"),
    };
    let pad = " ".repeat(indent);
    let mut out = format!("{header}\n");
    for line in body.split('\n') {
        if line.is_empty() {
            out.push('\n');
        } else {
            let _ = writeln!(out, "{pad}{line}");
        }
    }
    Some(out)
}

fn folded(text: &str, indent: usize) -> Option<String> {
    let plain = !text.is_empty()
        && text == text.trim()
        && !text.contains("  ")
        && !text.chars().any(char::is_control);
    if !plain {
        return None;
    }
    let pad = " ".repeat(indent);
    let width = WIDTH.saturating_sub(indent).max(20);
    let mut out = String::from(">-\n");
    let mut line = String::new();
    for word in text.split(' ') {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            let _ = writeln!(out, "{pad}{line}");
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    let _ = writeln!(out, "{pad}{line}");
    Some(out)
}

fn flow(items: &[Value]) -> String {
    let parts: Vec<String> = items.iter().map(scalar_or_empty).collect();
    format!("[{}]", parts.join(", "))
}

/// A list of scalars that fits on the line it starts on.
fn flow_fits(items: &[Value], indent: usize) -> bool {
    items.iter().all(|v| match v {
        Value::String(s) => !s.contains('\n'),
        Value::Mapping(_) | Value::Sequence(_) | Value::Tagged(_) => false,
        _ => true,
    }) && flow(items).chars().count() + indent <= WIDTH
}

fn scalar_or_empty(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => scalar_text(s),
        Value::Sequence(_) => "[]".to_owned(),
        Value::Mapping(_) => "{}".to_owned(),
        Value::Tagged(tagged) => scalar_or_empty(&tagged.value),
    }
}

fn key_text(key: &Value) -> String {
    match key {
        Value::String(text) => scalar_text(text),
        other => scalar_or_empty(other),
    }
}

/// A one-line scalar: plain when it reads back as the same string, else
/// double-quoted.
fn scalar_text(text: &str) -> String {
    if is_plain(text) {
        text.to_owned()
    } else {
        serde_json::Value::String(text.to_owned()).to_string()
    }
}

fn is_plain(text: &str) -> bool {
    const RESERVED: [&str; 8] = ["true", "false", "null", "yes", "no", "on", "off", "y"];
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    if RESERVED.contains(&lower.as_str()) || lower == "n" || text.ends_with(' ') {
        return false;
    }
    text.chars().all(|c| {
        c.is_alphanumeric()
            || matches!(
                c,
                '_' | '-' | '.' | '/' | ' ' | '$' | '(' | ')' | '+' | '@' | '='
            )
    }) && !text.contains("  ")
        && !text.contains(" #")
}
