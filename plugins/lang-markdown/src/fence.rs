//! The info string of a fenced code block, read as rustdoc and mdBook read it:
//! words separated by spaces and commas, the first the language and the rest
//! its attributes (`rust,ignore`, `rust no_run`, `rust edition2021`). Which
//! language a name stands for is not for the Markdown provider to say: the
//! languages declare their own names.

/// What a fence's info string says.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Info {
    /// The first word, as written.
    pub(crate) lang: String,
    /// The words after it, in order.
    pub(crate) attributes: Vec<String>,
}

impl Info {
    pub(crate) fn of(info: &str) -> Self {
        let mut words = info
            .split(|ch: char| ch.is_whitespace() || ch == ',')
            .filter(|word| !word.is_empty());
        let lang = words.next().unwrap_or_default().to_owned();
        let attributes = words.map(str::to_owned).collect();
        Self { lang, attributes }
    }
}
