//! What the decisions say of an edit, from the findings it adds and the
//! analysis that is incomplete.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use lighthouse_model::{Diagnostic, Incomplete, Severity};
use schemars::JsonSchema;
use serde::Serialize;

/// What the decisions say of the edit. Open-world: `permitted` is that no
/// enabled decision fires within what ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    /// A decision at `error` or `warn` fires on the edit.
    Forbidden,
    /// None does, or only at `info`.
    Permitted,
    /// None fires that is certain, and some analysis is incomplete.
    Unknown,
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Forbidden => "forbidden",
            Self::Permitted => "permitted",
            Self::Unknown => "unknown",
        })
    }
}

impl Verdict {
    /// `forbidden` when a finding of `error` or `warn` appears that no gap leaves
    /// in doubt; `unknown` when one appears that a gap leaves in doubt, or the
    /// analysis of what is reported with the edit is incomplete; else `permitted`.
    /// A gap of the run without the edit only puts the findings it covers in
    /// doubt: they may have been there already.
    pub(super) fn of(
        appear: &[&Diagnostic],
        (before, after): (&[Incomplete], &[Incomplete]),
        (root, scope): (&Path, &[PathBuf]),
    ) -> Verdict {
        let relative = |gap: &Incomplete| {
            gap.path
                .as_ref()
                .map(|path| path.strip_prefix(root).unwrap_or(path).to_owned())
        };
        let covers =
            |gap: &Incomplete, file: &Path| relative(gap).is_none_or(|path| file.starts_with(path));
        let loud: Vec<&&Diagnostic> = appear
            .iter()
            .filter(|d| matches!(d.severity, Severity::Error | Severity::Warn))
            .collect();
        if loud
            .iter()
            .any(|d| !before.iter().chain(after).any(|gap| covers(gap, &d.file)))
        {
            return Verdict::Forbidden;
        }
        let touches = |gap: &Incomplete| {
            relative(gap).is_none_or(|path| {
                scope
                    .iter()
                    .any(|s| s.starts_with(&path) || path.starts_with(s))
            })
        };
        if !loud.is_empty() || after.iter().any(touches) {
            Verdict::Unknown
        } else {
            Verdict::Permitted
        }
    }
}
