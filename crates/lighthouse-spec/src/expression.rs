//! The CEL expressions and `{{ cel }}` templates a decision's check and fix
//! carry: split once, and described when they do not hold.

use crate::environment::compile;

/// One part of a `{{ cel }}` template: literal text, or the source of a hole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplatePart {
    Text(String),
    Hole(String),
}

/// Splits a template into its text and its `{{ cel }}` holes; the one parser
/// every template of a decision (check messages, fix names and texts) goes
/// through. It does not compile the holes.
pub fn parse_template(text: &str) -> Result<Vec<TemplatePart>, String> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let (before, after) = rest.split_at(open);
        if !before.is_empty() {
            parts.push(TemplatePart::Text(before.to_owned()));
        }
        let Some(close) = after.find("}}") else {
            return Err("`{{` is never closed".to_owned());
        };
        parts.push(TemplatePart::Hole(after[2..close].trim().to_owned()));
        rest = &after[close + 2..];
    }
    if !rest.is_empty() {
        parts.push(TemplatePart::Text(rest.to_owned()));
    }
    Ok(parts)
}

/// Why `source`, written as `what`, is not a CEL expression.
pub(crate) fn compile_problem(what: &str, source: &str) -> Option<String> {
    compile(source).err().map(|e| format!("{what}: {e}"))
}

/// Why the `{{ cel }}` template `text`, written as `what`, is not well formed.
pub(crate) fn template_problem(what: &str, text: &str) -> Option<String> {
    match parse_template(text) {
        Err(problem) => Some(format!("{what}: {problem}")),
        Ok(parts) => parts.iter().find_map(|part| match part {
            TemplatePart::Hole(source) => compile_problem(&format!("{what}: `{source}`"), source),
            TemplatePart::Text(_) => None,
        }),
    }
}
