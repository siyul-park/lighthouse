//! The info string of a fenced code block, read as rustdoc and mdBook read it:
//! words separated by spaces and commas, the first the language and the rest
//! its attributes (`rust,ignore`, `rust no_run`, `rust edition2021`).

/// The provider key of each label a fence may carry, by lower-case label. A
/// label not listed is its own lower-case self, a key no provider has unless
/// one is registered under it.
const KEYS: &[(&str, &str)] = &[
    ("cel", "cel"),
    ("go", "go"),
    ("golang", "go"),
    ("rs", "rust"),
    ("rust", "rust"),
    ("toml", "toml"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
];

/// What a fence's info string says.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Info {
    /// The first word, as written.
    pub(crate) lang: String,
    /// The words after it, in order.
    pub(crate) attributes: Vec<String>,
    /// The key of the provider that analyzes the fence; empty without a
    /// language.
    pub(crate) key: String,
}

impl Info {
    pub(crate) fn of(info: &str) -> Self {
        let mut words = info
            .split(|ch: char| ch.is_whitespace() || ch == ',')
            .filter(|word| !word.is_empty());
        let lang = words.next().unwrap_or_default().to_owned();
        let attributes = words.map(str::to_owned).collect();
        let lower = lang.to_lowercase();
        let key = KEYS
            .iter()
            .find(|(label, _)| *label == lower)
            .map_or(lower, |(_, key)| (*key).to_owned());
        Self {
            lang,
            attributes,
            key,
        }
    }
}
