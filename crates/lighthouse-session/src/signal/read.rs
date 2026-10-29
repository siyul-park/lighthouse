//! What a prompt says, ready to record: the class and rule, a scrubbed
//! excerpt, the features that are not text, and the id of a signal.

use lighthouse_model::SignalClass;
use lighthouse_protocol::hash;
use serde::Serialize;
use serde_json::{Value, json};

use super::{
    classify::{read, truncated},
    scrub::scrub,
    strip::Stripped,
};

/// The prefix of a `provenance.wasDerivedFrom` entry that names a signal.
pub const DERIVED_PREFIX: &str = "signal:";

/// The rule id of a message that no rule decides.
pub const ABSTAIN_RULE: &str = "";

/// A signal as the classifier hands it on, before it is recorded.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Said {
    pub class: SignalClass,
    pub rule: &'static str,
    /// The sentence the rule fired on, secrets hidden, at most
    /// [`EXCERPT_CHARS`] characters.
    pub excerpt: String,
    pub features: Value,
}

/// The id of a signal: the first 12 hex digits of the hash of what makes it
/// the same signal. Those who see one thing twice pass the same parts.
pub fn signal_id(parts: &[&str]) -> String {
    hash::short(&parts.join("\u{0}"), 6)
}

/// What is measured of a message, never its text: the language, the size, and
/// whether code or pasted content was cut out.
pub fn features(prompt: &str, stripped: &Stripped) -> Value {
    let hangul = stripped
        .text
        .chars()
        .filter(|c| ('\u{AC00}'..='\u{D7A3}').contains(c))
        .count();
    let latin = stripped
        .text
        .chars()
        .filter(char::is_ascii_alphabetic)
        .count();
    let language = match (hangul, latin) {
        (0, 0) => "none",
        (h, l) if h * 10 >= (h + l) * 6 => "ko",
        (h, l) if h * 10 < (h + l) * 2 => "en",
        _ => "mixed",
    };
    json!({
        "language": language,
        "chars": prompt.chars().count(),
        "hadCode": stripped.had_code,
        "hadPasted": stripped.had_pasted,
    })
}

/// What a prompt says about a design decision: the class a rule proves, or
/// `abstain` when none does and no model is bound to answer.
pub fn said(prompt: &str) -> Said {
    let reading = read(prompt);
    let features = features(prompt, &reading.stripped);
    match reading.hit {
        Some(hit) => Said {
            class: hit.class,
            rule: hit.rule,
            excerpt: truncated(&scrub(&hit.sentence)),
            features,
        },
        None => Said {
            class: SignalClass::Abstain,
            rule: ABSTAIN_RULE,
            excerpt: truncated(&scrub(&reading.stripped.text)),
            features,
        },
    }
}
