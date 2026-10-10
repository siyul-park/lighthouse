//! The words of a SARIF result: its message, and the form of the message
//! that identifies a finding.

use super::log::{Message, ReportingDescriptor};

/// The text of a message: `text`, else the rule's `messageStrings[id]` with
/// its `{n}` placeholders filled from `arguments`, else the rule id.
pub(super) fn message(
    message: &Message,
    rule: Option<&ReportingDescriptor>,
    rule_id: &str,
) -> String {
    let text = message.text.clone().or_else(|| {
        let key = message.id.as_ref()?;
        let template = &rule?.message_strings.get(key)?.text;
        Some(fill(template, &message.arguments))
    });
    text.map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| rule_id.to_owned())
}

/// `template` with each `{n}` replaced by argument `n`; a placeholder with no
/// argument stays as written.
pub fn fill(template: &str, arguments: &[String]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open + 1..];
        let argument = tail
            .split_once('}')
            .and_then(|(n, after)| Some((arguments.get(n.parse::<usize>().ok()?)?, after)));
        match argument {
            Some((value, after)) => {
                out.push_str(value);
                rest = after;
            }
            None => {
                out.push('{');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The message without the numbers in it, which move when code does: each run
/// of digits becomes `#`. Names stay, so two findings of one rule about
/// different callees are different findings.
pub fn identifying(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_ascii_digit() {
            while chars.next_if(char::is_ascii_digit).is_some() {}
            out.push('#');
        } else {
            out.push(ch);
        }
    }
    out
}
