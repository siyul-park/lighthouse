//! How Claude Code marks a paste in a prompt: `<pasted_content id="…">` up to
//! its closing tag (or the end, when the tag is missing) and the placeholder
//! `[Pasted text #1 +20 lines]` of a collapsed one. The session does not know
//! this format, so the adapter turns every paste into a quoted line, which the
//! session already cuts out and counts as pasted.

use std::{borrow::Cow, sync::LazyLock};

use regex::Regex;

/// What stands for a paste: a quote the session cuts like any other.
const QUOTE: &str = "\n> \n";

static PASTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)<pasted_content\b[^>]*>.*?(?:</pasted_content\b[^>]*>|\z)|\[(?:Pasted text|Image|\.\.\.Truncated)[^\]]*\]",
    )
    .expect("paste")
});

/// The prompt with every paste a quoted line.
pub fn quoted(prompt: &str) -> Cow<'_, str> {
    PASTE.replace_all(prompt, QUOTE)
}
