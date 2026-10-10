//! Identifier words, for the facts about names.

/// Lower-case words of an identifier: split at `_`, at a lower-to-upper
/// change, and before the last capital of an acronym followed by a word
/// (`SDKThing` is `sdk`, `thing`).
pub(crate) fn words(identifier: &str) -> Vec<String> {
    let chars: Vec<char> = identifier.chars().collect();
    let mut words = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' {
            flush(&mut words, &mut current);
            continue;
        }
        let after_lower = i > 0 && chars[i - 1].is_lowercase() && c.is_uppercase();
        let acronym_end = i > 0
            && chars[i - 1].is_uppercase()
            && c.is_uppercase()
            && chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        if after_lower || acronym_end {
            flush(&mut words, &mut current);
        }
        current.extend(c.to_lowercase());
    }
    flush(&mut words, &mut current);
    words
}

/// The words of an identifier joined by `_`: two names share a prefix of
/// words exactly when one key starts with the other followed by `_`.
pub(crate) fn key(identifier: &str) -> String {
    words(identifier).join("_")
}

fn flush(words: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        words.push(std::mem::take(current));
    }
}
