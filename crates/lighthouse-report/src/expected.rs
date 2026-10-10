use std::path::Path;

use lighthouse_spec::{Decision, Example, ExampleKind};
use serde::Serialize;
use serde_json::Value;

/// Longest excerpt of an example, in lines and in characters.
const EXCERPT_LINES: usize = 12;
const EXCERPT_CHARS: usize = 600;

/// A short picture of what the code should look like.
#[derive(Serialize)]
pub(crate) struct Expected {
    /// `example`: a valid example of the decision.
    pub(crate) source: &'static str,
    pub(crate) language: String,
    /// Why this one: `canonical`, `matches kind=function` or `shortest valid
    /// example`.
    pub(crate) basis: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<String>,
    pub(crate) excerpt: String,
}

/// What the expected structure is chosen by: the file's language and the kind
/// and visibility of the finding's symbol.
pub(crate) struct Subject<'a> {
    pub(crate) language: Option<&'a str>,
    pub(crate) kind: Option<&'a str>,
    pub(crate) visibility: Option<&'a str>,
}

impl<'a> Subject<'a> {
    pub(crate) fn of(facts: Option<&'a Value>) -> Self {
        let text = |key: &str| facts.and_then(|f| f.get(key)).and_then(Value::as_str);
        Self {
            language: text("language"),
            kind: text("kind"),
            visibility: text("visibility"),
        }
    }
}

/// The canonical valid example for the file's language; else the valid
/// example whose name (or whose invalid counterpart's) mentions the kind or
/// visibility of the finding's symbol; else the shortest valid example; else
/// the decision's tuning note for the language. Each kept short.
pub(crate) fn expected(decision: &Decision, subject: &Subject, file: &Path) -> Option<Expected> {
    let valid: Vec<&Example> = decision
        .examples
        .iter()
        .filter(|e| e.kind == ExampleKind::Valid)
        .filter(|e| subject.language.is_none_or(|l| e.language == l))
        .collect();
    let chosen = valid
        .iter()
        .find(|e| e.canonical)
        .map(|e| (*e, "canonical".to_owned()))
        .or_else(|| matching(decision, &valid, subject))
        .or_else(|| {
            let shortest = valid.iter().min_by_key(|e| size(e))?;
            Some((*shortest, "shortest valid example".to_owned()))
        });
    if let Some((example, basis)) = chosen {
        return Some(from_example(example, basis, file));
    }
    None
}

/// The valid example that mentions the most of the subject's kind and
/// visibility, in its own name or in its invalid counterpart's; none when no
/// example mentions either.
pub(crate) fn matching<'a>(
    decision: &'a Decision,
    valid: &[&'a Example],
    subject: &Subject,
) -> Option<(&'a Example, String)> {
    let score = |example: &Example| {
        let counterpart = decision
            .examples
            .iter()
            .filter(|e| e.kind == ExampleKind::Invalid && e.language == example.language)
            .find(|e| pair_key(&e.name) == pair_key(&example.name))
            .map_or("", |e| e.name.as_str());
        let names = format!("{} {counterpart}", example.name).to_lowercase();
        [subject.kind, subject.visibility]
            .into_iter()
            .flatten()
            .filter(|word| names.contains(*word))
            .count()
    };
    let best = valid
        .iter()
        .map(|e| (score(e), *e))
        .filter(|(score, _)| *score > 0)
        .max_by_key(|(score, _)| *score)?
        .1;
    let kind = subject.kind.unwrap_or("symbol");
    Some((best, format!("matches kind={kind}")))
}

/// A name without the words that mark an example valid or invalid, so
/// `rust-valid` and `rust-invalid` pair up.
pub(crate) fn pair_key(name: &str) -> String {
    name.replace("invalid", "").replace("valid", "")
}

pub(crate) fn size(example: &Example) -> usize {
    example.files.iter().map(|f| f.text().len()).sum()
}

pub(crate) fn from_example(example: &Example, basis: String, file: &Path) -> Expected {
    let extension = file.extension();
    let chosen = example
        .files
        .iter()
        .find(|f| Path::new(&f.path).extension() == extension)
        .or(example.files.first());
    Expected {
        source: "example",
        language: example.language.clone(),
        basis,
        name: Some(example.name.clone()),
        path: chosen.map(|f| f.path.clone()),
        excerpt: chosen.map(|f| excerpt(f.text())).unwrap_or_default(),
    }
}

pub(crate) fn excerpt(text: &str) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let kept = lines[..lines.len().min(EXCERPT_LINES)].join("\n");
    let cut = truncate(&kept, EXCERPT_CHARS);
    if lines.len() > EXCERPT_LINES && cut == kept {
        format!("{cut}\n...")
    } else {
        cut
    }
}

pub(crate) fn truncate(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_owned(),
    }
}

pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
