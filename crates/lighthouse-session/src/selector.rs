//! Naming decisions and findings: `--decision` patterns and fingerprint
//! prefixes, the same everywhere.

use globset::{GlobBuilder, GlobMatcher};
use lighthouse_model::Fingerprint;

use crate::Result;

/// Characters that make a decision pattern a glob and not an id.
const GLOB_CHARS: [char; 4] = ['*', '?', '[', '{'];

/// What `--decision` selects: ids, and globs such as `design/*` over them.
pub struct Selector {
    patterns: Vec<Pattern>,
}

struct Pattern {
    text: String,
    glob: Option<GlobMatcher>,
}

impl Selector {
    /// The selector of these patterns; none selects everything.
    pub fn new(patterns: &[String]) -> Result<Self> {
        let patterns = patterns
            .iter()
            .map(|text| {
                let glob = if text.contains(GLOB_CHARS) {
                    let glob = GlobBuilder::new(text)
                        .literal_separator(true)
                        .build()
                        .map_err(|e| format!("decision pattern `{text}`: {e}"))?;
                    Some(glob.compile_matcher())
                } else {
                    None
                };
                Ok(Pattern {
                    text: text.clone(),
                    glob,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self { patterns })
    }

    /// Whether nothing narrows the selection.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether `id` is selected.
    pub fn matches(&self, id: &str) -> bool {
        self.patterns.is_empty() || self.patterns.iter().any(|p| p.matches(id))
    }

    /// The ids the patterns name: an id as it is, a glob as the `known` ids it
    /// matches. A glob that matches none is an error, because it names a
    /// decision that does not exist.
    pub fn expand<'a>(&self, known: impl IntoIterator<Item = &'a str>) -> Result<Vec<String>> {
        let known: Vec<&str> = known.into_iter().collect();
        let mut ids = Vec::new();
        for pattern in &self.patterns {
            let Some(glob) = &pattern.glob else {
                ids.push(pattern.text.clone());
                continue;
            };
            let before = ids.len();
            ids.extend(
                known
                    .iter()
                    .filter(|id| glob.is_match(id))
                    .map(|id| (*id).to_owned()),
            );
            if ids.len() == before {
                return Err(format!("no decision matches `{}`", pattern.text).into());
            }
        }
        ids.sort();
        ids.dedup();
        Ok(ids)
    }
}

impl Pattern {
    fn matches(&self, id: &str) -> bool {
        self.glob
            .as_ref()
            .map_or_else(|| self.text == id, |glob| glob.is_match(id))
    }
}

/// Refuses a fingerprint prefix shorter than [`Fingerprint::MIN_PREFIX`].
pub(crate) fn long_enough(prefix: &str) -> Result<()> {
    if prefix.len() < Fingerprint::MIN_PREFIX {
        return Err(lighthouse_store::Error::ShortFingerprint(prefix.to_owned()).into());
    }
    Ok(())
}
