//! The `decision-signal` classifier: does a person's message state a design
//! decision, and which kind? Rules decide what they can prove; for every other
//! message the classifier abstains and a model, when the project binds one for
//! classification, may answer. With no binding an abstention stays one.

use std::sync::LazyLock;

use lighthouse_model::SignalClass;
use regex::{Regex, RegexBuilder};
use serde::Deserialize;

use super::strip::{Stripped, stripped};

const RULES: &str = include_str!("rules.toml");
/// Longest excerpt kept of a message, in characters.
pub const EXCERPT_CHARS: usize = 200;

static TABLE: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let table: Table = toml::from_str(RULES).expect("rules.toml is valid");
    table
        .rule
        .into_iter()
        .map(|entry| Rule {
            pattern: RegexBuilder::new(&entry.pattern)
                .case_insensitive(true)
                .build()
                .unwrap_or_else(|e| panic!("rule `{}`: {e}", entry.id)),
            // The table lives as long as the process.
            id: Box::leak(entry.id.into_boxed_str()),
            class: entry.class,
        })
        .collect()
});

/// A Korean sentence that asks.
static ASKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:까|나요|가요|니|냐|까요)\s*$").expect("asks"));

#[derive(Deserialize)]
struct Table {
    rule: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    class: SignalClass,
    pattern: String,
}

struct Rule {
    id: &'static str,
    class: SignalClass,
    pattern: Regex,
}

/// A message read: what of it was the person's words, and what the rules
/// made of it. `hit` is `None` when the rules abstain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub stripped: Stripped,
    pub hit: Option<Hit>,
}

/// What a rule found in a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub class: SignalClass,
    /// The id of the rule that fired.
    pub rule: &'static str,
    /// The sentence it fired on, as written.
    pub sentence: String,
}

/// Reads a prompt: cuts out code, pasted content and quoted logs, then
/// matches the rest sentence by sentence.
pub fn read(prompt: &str) -> Reading {
    let stripped = stripped(prompt);
    let hit = classify(&stripped.text);
    Reading { stripped, hit }
}

/// The rule that decides `text`, which holds only the person's words: the
/// first rule, in the order of the table, that matches a sentence that is not
/// a question.
pub fn classify(text: &str) -> Option<Hit> {
    let sentences: Vec<&str> = sentences(text).into_iter().filter(|s| !asks(s)).collect();
    // A rule that proves a message says nothing only counts when no rule
    // finds a decision in it.
    let (nones, decisions): (Vec<&Rule>, Vec<&Rule>) = TABLE
        .iter()
        .partition(|rule| rule.class == SignalClass::None);
    decisions.into_iter().chain(nones).find_map(|rule| {
        sentences
            .iter()
            .find(|s| rule.pattern.is_match(s))
            .map(|s| Hit {
                class: rule.class,
                rule: rule.id,
                sentence: (*s).to_owned(),
            })
    })
}

/// The ids of every rule, in priority order.
pub fn rules() -> impl Iterator<Item = (&'static str, SignalClass)> {
    TABLE.iter().map(|rule| (rule.id, rule.class))
}

/// `text` cut to at most [`EXCERPT_CHARS`] characters, with an ellipsis when
/// it was cut.
pub fn truncated(text: &str) -> String {
    let squashed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if squashed.chars().count() <= EXCERPT_CHARS {
        return squashed;
    }
    let cut: String = squashed.chars().take(EXCERPT_CHARS - 1).collect();
    format!("{cut}…")
}

fn asks(sentence: &str) -> bool {
    let end = sentence.trim_end();
    end.ends_with(['?', '？']) || ASKS.is_match(end)
}

/// The sentences of `text`: a terminator followed by white space or the end
/// closes one, and so does a line break.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let end = match c {
            '\n' => Some(at),
            '.' | '!' | '?' | '？' | '。' => {
                let mut last = at + c.len_utf8();
                while let Some(&(next, n)) = chars.peek() {
                    if matches!(n, '.' | '!' | '?' | '？') {
                        last = next + n.len_utf8();
                        chars.next();
                    } else {
                        break;
                    }
                }
                chars
                    .peek()
                    .is_none_or(|&(_, n)| n.is_whitespace())
                    .then_some(last)
            }
            _ => None,
        };
        if let Some(end) = end {
            out.push(&text[start..end]);
            start = end;
        }
    }
    out.push(&text[start..]);
    out.into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}
