//! JSON in its canonical form, RFC 8785 (JCS): members sorted by their UTF-16
//! code units, no insignificant whitespace, numbers as ECMAScript prints
//! them. The same value is the same bytes, so a digest of it identifies it.

use std::fmt::Write;

use serde_json::Value;

/// `value` as RFC 8785 canonical JSON.
pub fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n)),
        Value::String(s) => string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut members: Vec<(&String, &Value)> = map.iter().collect();
            members.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
            out.push('{');
            for (i, (key, member)) in members.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                string(out, key);
                out.push(':');
                write_value(out, member);
            }
            out.push('}');
        }
    }
}

fn string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < '\u{20}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A number as ECMAScript's `Number::toString` prints it: integers without a
/// fraction, an exponent only below 1e-6 or from 1e21.
fn number(n: &serde_json::Number) -> String {
    let Some(x) = n.as_f64().filter(|_| n.is_f64()) else {
        return n.to_string();
    };
    if x == 0.0 {
        return "0".to_owned();
    }
    let magnitude = x.abs();
    if (1e-6..1e21).contains(&magnitude) {
        // Rust prints the shortest digits that read back, with no exponent.
        return format!("{x}");
    }
    let scientific = format!("{x:e}");
    match scientific.split_once('e') {
        Some((digits, exponent)) if !exponent.starts_with('-') => format!("{digits}e+{exponent}"),
        _ => scientific,
    }
}
