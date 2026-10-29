//! The text of a string scalar, and the host bytes each of its bytes stands
//! for: plain, quoted (with escapes and line folding) and block scalars.

use lighthouse_protocol::TextOffsets;
/// The decoded text of a scalar, with where each byte came from.
pub(crate) struct MappedScalar {
    pub(crate) text: String,
    pub(crate) offsets: TextOffsets,
}

pub(crate) fn map_scalar(
    host: &str,
    raw: &str,
    start: usize,
    value: &str,
) -> Result<MappedScalar, String> {
    let leading = raw.len() - raw.trim_start().len();
    let trim = raw.trim();
    let at = start + leading;
    if trim == value {
        let mut offsets = TextOffsets::with_capacity(value.len());
        for (offset, ch) in trim.char_indices() {
            offsets.push(ch, at + offset, at + offset + ch.len_utf8());
        }
        return Ok(mapped(value, offsets));
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
        return body_map(raw, start, value);
    }
    Err(format!(
        "cannot map decoded scalar to exact host bytes: raw={trim:?}, value={value:?}, span={start}..{}",
        start + raw.len()
    ))
}

fn mapped(text: &str, offsets: TextOffsets) -> MappedScalar {
    MappedScalar {
        text: text.to_owned(),
        offsets,
    }
}

fn quoted_map(raw: &str, start: usize, value: &str) -> Result<MappedScalar, String> {
    let bytes = raw.as_bytes();
    let quote = bytes[0];
    let mut offsets = TextOffsets::with_capacity(value.len());
    let mut raw_at = 1;
    let raw_end = raw.len() - 1;
    for ch in value.chars() {
        if raw_at >= raw_end {
            return Err("quoted scalar ends before its decoded value".to_owned());
        }
        let from = raw_at;
        if quote == b'\'' && bytes.get(raw_at..raw_at + 2) == Some(b"''") {
            raw_at += 2;
        } else if quote == b'"' && bytes[raw_at] == b'\\' {
            if !quoted_escape(raw, &mut raw_at, raw_end)? {
                continue;
            }
        } else if matches!(bytes[raw_at], b'\n' | b'\r') {
            quoted_fold(raw, &mut raw_at, raw_end, ch)?;
        } else {
            let raw_char = raw
                .get(raw_at..)
                .and_then(|tail| tail.chars().next())
                .ok_or_else(|| "invalid UTF-8 in scalar".to_owned())?;
            if raw_char != ch {
                return Err("quoted scalar source map does not match decoded value".to_owned());
            }
            raw_at += raw_char.len_utf8();
        }
        offsets.push(ch, start + from, start + raw_at);
    }
    if raw_at != raw_end {
        let remaining = raw
            .get(raw_at..raw_end)
            .ok_or_else(|| "quoted scalar has unmapped trailing source bytes".to_owned())?;
        if !remaining.chars().all(char::is_whitespace) {
            return Err("quoted scalar has unmapped trailing source bytes".to_owned());
        }
    }
    Ok(mapped(value, offsets))
}

fn quoted_fold(raw: &str, raw_at: &mut usize, raw_end: usize, ch: char) -> Result<(), String> {
    let bytes = raw.as_bytes();
    let newline = *bytes
        .get(*raw_at)
        .ok_or_else(|| "quoted scalar ends before its decoded value".to_owned())?;
    *raw_at += 1;
    if newline == b'\r' && bytes.get(*raw_at) == Some(&b'\n') {
        *raw_at += 1;
    }
    while *raw_at < raw_end && matches!(bytes[*raw_at], b' ' | b'\t') {
        *raw_at += 1;
    }
    if !matches!(ch, ' ' | '\n') {
        return Err("quoted scalar folding does not match decoded value".to_owned());
    }
    Ok(())
}

/// Moves `raw_at` past the escape sequence it stands at. False when the
/// sequence decodes to no character (an escaped line break).
fn quoted_escape(raw: &str, raw_at: &mut usize, raw_end: usize) -> Result<bool, String> {
    let bytes = raw.as_bytes();
    *raw_at += 1;
    let escape = *bytes
        .get(*raw_at)
        .ok_or_else(|| "unfinished YAML escape".to_owned())?;
    *raw_at += 1;
    let digits = match escape {
        b'0' | b'a' | b'b' | b't' | b'\t' | b'v' | b'n' | b'f' | b'r' | b'e' | b' ' | b'"'
        | b'/' | b'\\' | b'N' | b'_' | b'L' | b'P' => 0,
        b'x' => 2,
        b'u' => 4,
        b'U' => 8,
        b'\n' | b'\r' => {
            if escape == b'\r' && bytes.get(*raw_at) == Some(&b'\n') {
                *raw_at += 1;
            }
            while *raw_at < raw_end && bytes[*raw_at].is_ascii_whitespace() {
                *raw_at += 1;
            }
            return Ok(false);
        }
        _ => return Err("unknown YAML escape sequence".to_owned()),
    };
    let hex = raw
        .get(*raw_at..*raw_at + digits)
        .filter(|hex| hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| "invalid YAML escape digits".to_owned())?;
    *raw_at += hex.len();
    Ok(true)
}

/// A multi-line plain scalar, whose line breaks fold into spaces: each decoded
/// character is the next one of the source that equals it, or the next white
/// space for white space.
fn body_map(raw: &str, start: usize, value: &str) -> Result<MappedScalar, String> {
    let mut cursor = 0;
    let mut offsets = TextOffsets::with_capacity(value.len());
    for ch in value.chars() {
        let rest = &raw[cursor..];
        let exact = rest.char_indices().find(|(_, candidate)| *candidate == ch);
        let found = exact.or_else(|| {
            ch.is_whitespace()
                .then(|| {
                    rest.char_indices()
                        .find(|(_, candidate)| candidate.is_whitespace())
                })
                .flatten()
        });
        let Some((relative, source_char)) = found else {
            return Err(format!("YAML decoded scalar cannot be mapped at {ch:?}"));
        };
        let from = cursor + relative;
        let to = from + source_char.len_utf8();
        offsets.push(ch, start + from, start + to);
        cursor = to;
    }
    Ok(mapped(value, offsets))
}

/// A literal (`|`) or folded (`>`) block scalar.
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
    let body = host
        .get(body_at..start + raw.len())
        .ok_or_else(|| "block scalar body is outside the host text".to_owned())?;
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
    let mut offsets = TextOffsets::with_capacity(value.len());
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
            let from = line_start + leading + offset;
            offsets.push(ch, from, from + ch.len_utf8());
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
            offsets.push(separator, newline_at, newline_at + 1);
        }
    }
    while text.len() > value.len() || !value.starts_with(&text) {
        if text.ends_with('\n') && !value.ends_with('\n') {
            text.pop();
            offsets.pop();
        } else {
            return Err("block scalar source map does not match YAML-decoded value".to_owned());
        }
    }
    if text != value {
        return Err("block scalar source map is incomplete".to_owned());
    }
    Ok(mapped(&text, offsets))
}
