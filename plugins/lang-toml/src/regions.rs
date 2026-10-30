//! Where the bytes of a decoded TOML string came from: basic and literal
//! strings, single-line and multi-line, with their escapes.

use std::ops::Range;

use lighthouse_protocol::TextOffsets;

struct Mapping<'a> {
    raw: &'a str,
    base: usize,
    end: usize,
    multiline: bool,
    value: &'a str,
    offsets: TextOffsets,
}

impl Mapping<'_> {
    /// The decoded character that the next source character stands for.
    fn next(&self) -> Option<char> {
        self.value
            .get(self.offsets.len()..)
            .and_then(|tail| tail.chars().next())
    }

    fn push(&mut self, from: usize, to: usize, ch: char) {
        self.offsets.push(ch, self.base + from, self.base + to);
    }

    /// Moves `at` past the escape sequence it stands at.
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
            .next()
            .ok_or_else(|| "TOML escape has no decoded character".to_owned())?;
        let from = *at;
        *at += consumed;
        self.push(from, *at, ch);
        Ok(())
    }
}

/// The offsets of the decoded `value` of the string whose token is `range` of
/// `host`.
pub(crate) fn map(host: &str, range: Range<usize>, value: &str) -> Result<TextOffsets, String> {
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
        // A line break right after the opening delimiter is not the text.
        if raw[at..].starts_with("\r\n") {
            at += 2;
        } else if raw[at..].starts_with('\n') || raw[at..].starts_with('\r') {
            at += 1;
        }
    }
    let mut mapping = Mapping {
        raw,
        base: range.start,
        end,
        multiline,
        value,
        offsets: TextOffsets::with_capacity(value.len()),
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
        at += if ch == '\r' && raw[at..].starts_with("\r\n") {
            2
        } else {
            ch.len_utf8()
        };
        let decoded = if ch == '\r' { '\n' } else { ch };
        if mapping.next() != Some(decoded) {
            return Err(format!(
                "TOML decoded value differs at source byte {}: expected {:?}",
                range.start + from,
                value.get(mapping.offsets.len()..)
            ));
        }
        mapping.push(from, at, decoded);
    }
    if mapping.offsets.len() != value.len() {
        return Err(format!(
            "TOML source map covered {} decoded bytes, expected {}",
            mapping.offsets.len(),
            value.len()
        ));
    }
    Ok(mapping.offsets)
}
