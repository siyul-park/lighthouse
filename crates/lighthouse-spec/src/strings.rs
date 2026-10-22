//! The strings extension of CEL that cel-rust leaves out, under the names and
//! the semantics of cel-go's `ext.Strings` (which follow Go's `strings`), so
//! that an expression reads as it would in any other CEL: `split(separator)`
//! and `split(separator, limit)`, `replace(old, new)` and
//! `replace(old, new, limit)`, and `trim()`.

use std::sync::Arc;

use cel::{
    Context, ExecutionError, FunctionContext, Value,
    extractors::{Arguments, This},
};

type Result<T> = std::result::Result<T, ExecutionError>;

/// Installs the functions of the extension in `context`.
pub fn strings(context: &mut Context<'static>) {
    context.add_function(
        "split",
        |ftx: &FunctionContext, This(text): This<Arc<String>>, Arguments(args): Arguments| {
            let (separator, limit) = match args.as_slice() {
                [separator] => (word(ftx, separator)?, -1),
                [separator, limit] => (word(ftx, separator)?, count(ftx, limit)?),
                _ => return Err(ftx.error("`split` takes a separator and an optional limit")),
            };
            Ok::<Value, ExecutionError>(list(split(&text, separator, limit)))
        },
    );
    context.add_function(
        "replace",
        |ftx: &FunctionContext, This(text): This<Arc<String>>, Arguments(args): Arguments| {
            let (old, new, limit) = match args.as_slice() {
                [old, new] => (word(ftx, old)?, word(ftx, new)?, -1),
                [old, new, limit] => (word(ftx, old)?, word(ftx, new)?, count(ftx, limit)?),
                _ => return Err(ftx.error("`replace` takes two strings and an optional limit")),
            };
            Ok::<Value, ExecutionError>(Value::String(Arc::new(replace(&text, old, new, limit))))
        },
    );
    context.add_function("trim", |This(text): This<Arc<String>>| {
        text.trim().to_owned()
    });
}

/// `text` cut at `separator`, at most `limit` pieces when it is not negative:
/// none for 0, the whole text for 1, and an empty separator cuts between the
/// characters.
fn split(text: &str, separator: &str, limit: i64) -> Vec<String> {
    let at_most = match limit {
        0 => return Vec::new(),
        n if n < 0 => usize::MAX,
        n => usize::try_from(n).unwrap_or(usize::MAX),
    };
    if separator.is_empty() {
        let mut pieces: Vec<String> = Vec::new();
        for (at, character) in text.char_indices() {
            if pieces.len() + 1 == at_most {
                pieces.push(text[at..].to_owned());
                return pieces;
            }
            pieces.push(character.to_string());
        }
        return pieces;
    }
    text.splitn(at_most, separator).map(str::to_owned).collect()
}

/// `text` with the first `limit` occurrences of `old` replaced (all of them
/// when it is negative, none for 0).
fn replace(text: &str, old: &str, new: &str, limit: i64) -> String {
    match limit {
        0 => text.to_owned(),
        n if n < 0 => text.replace(old, new),
        n => text.replacen(old, new, usize::try_from(n).unwrap_or(usize::MAX)),
    }
}

fn list(pieces: Vec<String>) -> Value {
    Value::List(Arc::new(
        pieces
            .into_iter()
            .map(|piece| Value::String(Arc::new(piece)))
            .collect(),
    ))
}

fn word<'a>(ftx: &FunctionContext, value: &'a Value) -> Result<&'a str> {
    match value {
        Value::String(text) => Ok(text.as_str()),
        other => Err(ftx.error(format!("expected a string, got {other:?}"))),
    }
}

fn count(ftx: &FunctionContext, value: &Value) -> Result<i64> {
    match value {
        Value::Int(n) => Ok(*n),
        other => Err(ftx.error(format!("expected an int limit, got {other:?}"))),
    }
}
