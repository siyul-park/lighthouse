use serde_json::Value;

/// A selector over a host provider's selectable tree. The host provider adds
/// source text and maps it to the original document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    parts: Vec<SelectorPart>,
}

/// One step in a selector path or a predicate applied to the current candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorPart {
    /// `..`: the current value and every value below it, parents first, in
    /// the order the tree states them.
    Descend,
    Key(String),
    Index(usize),
    Keys,
    Indices,
    Filter(SelectorFilter),
}

/// Disjunction of conjunctions in a filter expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorFilter {
    pub clauses: Vec<Vec<SelectorComparison>>,
}

/// Scalar equality or inequality comparison against a candidate-relative path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorComparison {
    pub path: Selector,
    pub equal: bool,
    pub value: Value,
}

impl Selector {
    /// Parses the bounded path/filter grammar shared by host providers.
    pub fn parse(source: &str) -> Result<Self, String> {
        let bytes = source.as_bytes();
        if bytes.first() != Some(&b'$') {
            return Err("selector must start with '$'".to_owned());
        }
        let mut parts = Vec::new();
        let mut at = 1;
        while at < bytes.len() {
            match bytes[at] {
                b'.' if bytes.get(at + 1) == Some(&b'.') => {
                    parts.push(SelectorPart::Descend);
                    at += 1;
                    match bytes.get(at + 1) {
                        Some(b'[' | b'?') => at += 1,
                        Some(_) => parts.push(parse_key(source, &mut at)?),
                        None => return Err("descendant segment needs a selector".to_owned()),
                    }
                }
                b'.' => parts.push(parse_key(source, &mut at)?),
                b'[' if bytes.get(at + 1) == Some(&b'?') => {
                    at += 1;
                    parts.push(SelectorPart::Filter(parse_filter(source, &mut at, true)?));
                }
                b'[' => parts.push(parse_bracket(source, &mut at)?),
                b'?' => parts.push(SelectorPart::Filter(parse_filter(source, &mut at, false)?)),
                _ => return Err(format!("unexpected selector character at byte {at}")),
            }
        }
        Ok(Self { parts })
    }

    /// Returns parsed selector steps in evaluation order.
    pub fn parts(&self) -> &[SelectorPart] {
        &self.parts
    }
}

fn parse_key(source: &str, at: &mut usize) -> Result<SelectorPart, String> {
    let bytes = source.as_bytes();
    *at += 1;
    let start = *at;
    if bytes.get(*at) == Some(&b'*') {
        *at += 1;
        if *at == bytes.len() || matches!(bytes[*at], b'.' | b'[' | b'?') {
            return Ok(SelectorPart::Keys);
        }
        return Err(format!(
            "wildcard must be a complete selector segment at byte {start}"
        ));
    }
    while *at < bytes.len() {
        let ch = source[*at..]
            .chars()
            .next()
            .ok_or("invalid UTF-8 selector")?;
        if matches!(ch, '.' | '[' | '?') {
            break;
        }
        if !(ch.is_alphanumeric() || matches!(ch, '_' | '-')) {
            return Err(format!("invalid character in selector key at byte {}", *at));
        }
        *at += ch.len_utf8();
    }
    if start == *at {
        return Err(format!("empty selector segment at byte {start}"));
    }
    let key = source
        .get(start..*at)
        .ok_or("invalid UTF-8 selector boundary")?;
    Ok(if key == "*" {
        SelectorPart::Keys
    } else {
        SelectorPart::Key(key.to_owned())
    })
}

fn parse_bracket(source: &str, at: &mut usize) -> Result<SelectorPart, String> {
    let bytes = source.as_bytes();
    *at += 1;
    match bytes.get(*at).copied() {
        Some(b'*') => {
            *at += 1;
            if bytes.get(*at) != Some(&b']') {
                return Err("wildcard selector must be '[*]'".to_owned());
            }
            *at += 1;
            Ok(SelectorPart::Indices)
        }
        Some(b'\'' | b'"') => parse_quoted_key(source, at),
        Some(b'0'..=b'9') => parse_index(source, at),
        None => Err("unterminated bracket selector".to_owned()),
        _ => Err("brackets accept '*', a numeric index, or a quoted key".to_owned()),
    }
}

fn parse_quoted_key(source: &str, at: &mut usize) -> Result<SelectorPart, String> {
    let bytes = source.as_bytes();
    let quote = bytes[*at];
    let start = *at;
    *at += 1;

    let key = if quote == b'"' {
        let mut escaped = false;
        while *at < bytes.len() {
            let byte = bytes[*at];
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == quote {
                break;
            }
            *at += 1;
        }
        if bytes.get(*at) != Some(&quote) {
            return Err("unterminated quoted key selector".to_owned());
        }
        *at += 1;
        serde_json::from_str::<String>(
            source
                .get(start..*at)
                .ok_or("invalid UTF-8 selector boundary")?,
        )
        .map_err(|_| "invalid escaped key in selector".to_owned())?
    } else {
        let mut key = String::new();
        while *at < bytes.len() && bytes[*at] != quote {
            if bytes[*at] == b'\\' {
                *at += 1;
                let escaped = source
                    .get(*at..)
                    .and_then(|tail| tail.chars().next())
                    .ok_or("unterminated quoted key escape")?;
                if !matches!(escaped, '\\' | '\'') {
                    return Err("single-quoted keys only escape backslash and quote".to_owned());
                }
                key.push(escaped);
                *at += escaped.len_utf8();
            } else {
                let ch = source[*at..]
                    .chars()
                    .next()
                    .ok_or("invalid UTF-8 selector")?;
                key.push(ch);
                *at += ch.len_utf8();
            }
        }
        if bytes.get(*at) != Some(&quote) {
            return Err("unterminated quoted key selector".to_owned());
        }
        *at += 1;
        key
    };
    if bytes.get(*at) != Some(&b']') {
        return Err("quoted key selector must end with ']'".to_owned());
    }
    *at += 1;
    Ok(SelectorPart::Key(key))
}

fn parse_index(source: &str, at: &mut usize) -> Result<SelectorPart, String> {
    let bytes = source.as_bytes();
    let start = *at;
    while *at < bytes.len() && bytes[*at].is_ascii_digit() {
        *at += 1;
    }
    if bytes.get(*at) != Some(&b']') {
        return Err("array index must end with ']'".to_owned());
    }
    let index = source[start..*at]
        .parse()
        .map_err(|_| "array index is too large")?;
    *at += 1;
    Ok(SelectorPart::Index(index))
}

fn parse_filter(source: &str, at: &mut usize, bracketed: bool) -> Result<SelectorFilter, String> {
    let bytes = source.as_bytes();
    *at += 1;
    if bytes.get(*at) != Some(&b'(') {
        return Err("filter must start with '?('".to_owned());
    }
    *at += 1;
    let start = *at;
    let mut depth = 1usize;
    let mut quote = None;
    let mut escaped = false;
    while *at < bytes.len() {
        let byte = bytes[*at];
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' && q != b'\'' {
                escaped = true;
            } else if byte == q {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if byte == b'(' {
            depth += 1;
        } else if byte == b')' {
            depth -= 1;
            if depth == 0 {
                break;
            }
        }
        *at += 1;
    }
    if depth != 0 {
        return Err("unterminated filter expression".to_owned());
    }
    let expression = &source[start..*at];
    *at += 1;
    if bracketed {
        if bytes.get(*at) != Some(&b']') {
            return Err("filter bracket must end with ']'".to_owned());
        }
        *at += 1;
    }
    parse_filter_expression(expression)
}

fn parse_filter_expression(source: &str) -> Result<SelectorFilter, String> {
    let mut clauses = Vec::new();
    for disjunction in split_expression(source, "||")? {
        let mut clause = Vec::new();
        for conjunction in split_expression(disjunction, "&&")? {
            clause.push(parse_comparison(conjunction.trim())?);
        }
        if clause.is_empty() {
            return Err("empty filter clause".to_owned());
        }
        clauses.push(clause);
    }
    if clauses.is_empty() {
        return Err("empty filter expression".to_owned());
    }
    Ok(SelectorFilter { clauses })
}

fn split_expression<'a>(source: &'a str, operator: &str) -> Result<Vec<&'a str>, String> {
    let bytes = source.as_bytes();
    let op = operator.as_bytes();
    let mut result = Vec::new();
    let mut start = 0;
    let mut at = 0;
    let mut quote = None;
    let mut escaped = false;
    while at < bytes.len() {
        let byte = bytes[at];
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' && q != b'\'' {
                escaped = true;
            } else if byte == q {
                quote = None;
            }
            at += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
            at += 1;
            continue;
        }
        if bytes[at..].starts_with(op) {
            result.push(source[start..at].trim());
            at += op.len();
            start = at;
        } else {
            at += 1;
        }
    }
    if quote.is_some() {
        return Err("unterminated string in filter".to_owned());
    }
    result.push(source[start..].trim());
    Ok(result)
}

fn parse_comparison(source: &str) -> Result<SelectorComparison, String> {
    let (at, operator, width) = find_comparison(source)
        .ok_or_else(|| "filter predicate must compare @.path using == or !=".to_owned())?;
    let left = source[..at].trim();
    let right = source[at + width..].trim();
    if !left.starts_with('@') {
        return Err("filter paths must start with '@'".to_owned());
    }
    let path = Selector::parse(&("$".to_owned() + &left[1..]))?;
    let value = parse_scalar(right)?;
    Ok(SelectorComparison {
        path,
        equal: operator == "==",
        value,
    })
}

fn find_comparison(source: &str) -> Option<(usize, &'static str, usize)> {
    let bytes = source.as_bytes();
    let mut at = 0;
    let mut quote = None;
    let mut escaped = false;
    while at < bytes.len() {
        let byte = bytes[at];
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' && q != b'\'' {
                escaped = true;
            } else if byte == q {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if bytes[at..].starts_with(b"==") {
            return Some((at, "==", 2));
        } else if bytes[at..].starts_with(b"!=") {
            return Some((at, "!=", 2));
        }
        at += 1;
    }
    None
}

fn parse_scalar(source: &str) -> Result<Value, String> {
    if source.starts_with('\'') && source.ends_with('\'') && source.len() >= 2 {
        return Ok(Value::String(
            source[1..source.len() - 1].replace("\\'", "'"),
        ));
    }
    if source.starts_with('"') {
        return serde_json::from_str(source)
            .map_err(|_| "invalid quoted string in filter".to_owned());
    }
    match source {
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        "null" => Ok(Value::Null),
        _ => {
            let number: serde_json::Number = serde_json::from_str(source).map_err(|_| {
                "filter values must be strings, numbers, booleans, or null".to_owned()
            })?;
            Ok(Value::Number(number))
        }
    }
}

#[cfg(test)]
mod tests;
