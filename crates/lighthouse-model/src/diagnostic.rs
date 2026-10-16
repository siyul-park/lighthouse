use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Project, Span,
    hash::{Hasher, short_digest},
    squash,
};

/// How hard a finding fails a run; serialized and parsed as the lowercase
/// variant name. Whether a finding also asks for review is not a level: it
/// follows from the severity its decision authored.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Fails the run.
    Error,
    /// Fails the run only under `--strict` or past `--max-warnings`.
    Warn,
    /// Reported, never fails the run.
    Info,
}

vocabulary!(Severity, "severity" {
    Error => "error",
    Warn => "warn",
    Info => "info",
});

impl Severity {
    /// Whether a finding of a decision that authored this severity asks for
    /// review: the check is not deterministic (an `error` is definitive: only a
    /// suppression waives it) and nobody has judged the finding yet.
    pub fn needs_review(self, judged: bool) -> bool {
        self != Self::Error && !judged
    }
}

/// Stable identity of a finding across runs and line shifts.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Fingerprint(String);

impl Fingerprint {
    /// Hashes the rule, the symbol path (separators normalized to `/`) and the
    /// whitespace-normalized snippet.
    pub fn of(rule_id: &str, symbol_path: &str, snippet: &str) -> Self {
        let symbol_path = symbol_path.replace('\\', "/");
        let mut hash = Hasher::new();
        for part in [rule_id, symbol_path.as_str()] {
            hash.update(part);
            hash.update([0]);
        }
        hash.update(squash(snippet));
        Self(hash.hex())
    }

    /// Adopts a fingerprint computed elsewhere, such as by a plugin.
    pub fn from_raw(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Distinguishes the `n`th (0-based) repeat of an otherwise identical finding.
    pub fn occurrence(&self, n: usize) -> Self {
        if n == 0 {
            return self.clone();
        }
        let mut hash = Hasher::new();
        hash.update(&self.0);
        hash.update(n.to_le_bytes());
        Self(hash.hex())
    }

    /// Tells apart findings that share a fingerprint by something stable about
    /// where they are, such as the symbol that encloses them.
    pub fn discriminate(&self, discriminator: &str) -> Self {
        let mut hash = Hasher::new();
        hash.update(&self.0);
        hash.update([0xff]);
        hash.update(discriminator);
        Self(hash.hex())
    }

    /// The opaque hex string; equal fingerprints identify the same finding.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One finding: where a rule found a violation and how to tell it apart from
/// the same finding in another run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub rule_id: String,
    pub severity: Severity,
    pub message: String,
    pub file: PathBuf,
    pub span: Span,
    pub fingerprint: Fingerprint,
    /// Id of the symbol the finding is about, when it is about one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub evidence: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// Why no check decided the subject: the diagnostic is a candidate for
    /// review, not a finding, and the engine reports it apart. The exit code
    /// does not count it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    /// A suppression the tool that reported the finding already carries, such
    /// as its own `//nolint`: the engine reports the finding as suppressed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppression: Option<Suppression>,
}

impl Diagnostic {
    /// A finding without symbol, evidence or fix; set those fields directly.
    pub fn new(
        rule_id: impl Into<String>,
        severity: Severity,
        message: impl Into<String>,
        file: impl Into<PathBuf>,
        span: Span,
        fingerprint: Fingerprint,
    ) -> Self {
        Self {
            rule_id: rule_id.into(),
            severity,
            message: message.into(),
            file: file.into(),
            span,
            fingerprint,
            symbol: None,
            evidence: Value::Null,
            fix: None,
            review: None,
            suppression: None,
        }
    }

    /// Makes the fingerprints of one run unique. Findings that share a rule,
    /// file and fingerprint are told apart by a stable discriminator (the
    /// finding's symbol, else the symbol that encloses or precedes it) and only
    /// findings that still collide get an ordinal. Returns the fingerprints
    /// that rest on an ordinal, which moves when an earlier identical finding
    /// appears or goes.
    pub fn disambiguate(found: &mut [Self], project: &Project) -> BTreeSet<Fingerprint> {
        let mut groups: BTreeMap<(&str, &PathBuf, &Fingerprint), Vec<usize>> = BTreeMap::new();
        for (at, d) in found.iter().enumerate() {
            groups
                .entry((&d.rule_id, &d.file, &d.fingerprint))
                .or_default()
                .push(at);
        }
        let groups: Vec<Vec<usize>> = groups.into_values().filter(|g| g.len() > 1).collect();
        let mut ordinal = BTreeSet::new();
        for members in groups {
            let mut by_discriminator: BTreeMap<String, Vec<usize>> = BTreeMap::new();
            for at in members {
                let discriminator = found[at].discriminator(project);
                by_discriminator.entry(discriminator).or_default().push(at);
            }
            for (discriminator, same) in by_discriminator {
                for (n, &at) in same.iter().enumerate() {
                    let base = &found[at].fingerprint;
                    found[at].fingerprint = if discriminator.is_empty() {
                        base.occurrence(n)
                    } else {
                        base.discriminate(&discriminator).occurrence(n)
                    };
                    if same.len() > 1 {
                        ordinal.insert(found[at].fingerprint.clone());
                    }
                }
            }
        }
        ordinal
    }

    /// The symbol the finding is about, else the one that encloses or precedes
    /// it; empty when there is none.
    fn discriminator(&self, project: &Project) -> String {
        if let Some(symbol) = &self.symbol {
            return symbol.clone();
        }
        let symbols: Vec<_> = project.symbols_in(&self.file).collect();
        let at = self.span.start;
        let innermost = symbols
            .iter()
            .filter(|s| s.span.start <= at && at <= s.span.end)
            .max_by_key(|s| s.span.start);
        let preceding = symbols
            .iter()
            .filter(|s| s.span.end < at)
            .max_by_key(|s| s.span.end);
        innermost
            .or(preceding)
            .map_or_else(String::new, |s| s.id.as_str().to_owned())
    }
}

/// A hash of evidence with whitespace in its strings collapsed, so
/// reformatting does not change what the evidence says.
pub fn evidence_digest(evidence: &Value) -> String {
    fn squashed(value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(squash(s)),
            Value::Array(items) => Value::Array(items.iter().map(squashed).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), squashed(value)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    short_digest(&squashed(evidence), 8)
}

/// Part of the analysis scope that could not be analyzed: a file, or the
/// whole run of a provider when `path` is `None`. Never silent: "not checked"
/// is not "passed".
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Incomplete {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub reason: String,
}

/// How a suppression was declared, in the words of SARIF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum SuppressionKind {
    /// By a directive in the code.
    #[serde(rename = "inSource")]
    InSource,
    /// Outside the code: a recorded decision to leave the finding in place.
    #[serde(rename = "external")]
    External,
}

vocabulary!(SuppressionKind, "suppression kind" {
    InSource => "inSource",
    External => "external",
});

/// Whether a suppression is in force, in the words of SARIF.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum SuppressionStatus {
    /// In force.
    #[default]
    #[serde(rename = "accepted")]
    Accepted,
    /// Proposed, not yet in force.
    #[serde(rename = "underReview")]
    UnderReview,
    /// Refused.
    #[serde(rename = "rejected")]
    Rejected,
}

vocabulary!(SuppressionStatus, "suppression status" {
    Accepted => "accepted",
    UnderReview => "underReview",
    Rejected => "rejected",
});

impl SuppressionStatus {
    fn is_default(&self) -> bool {
        *self == Self::Accepted
    }
}

/// A finding that is right but is left in place on purpose: SARIF's
/// `suppression`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Suppression {
    pub kind: SuppressionKind,
    /// `accepted` unless said otherwise.
    #[serde(default, skip_serializing_if = "SuppressionStatus::is_default")]
    pub status: SuppressionStatus,
    /// Why the finding stays.
    pub justification: String,
}

impl Suppression {
    /// An accepted suppression a directive in the code declares.
    pub fn in_source(justification: impl Into<String>) -> Self {
        Self::new(SuppressionKind::InSource, justification)
    }

    /// An accepted suppression recorded outside the code.
    pub fn external(justification: impl Into<String>) -> Self {
        Self::new(SuppressionKind::External, justification)
    }

    /// An accepted suppression of the given kind.
    fn new(kind: SuppressionKind, justification: impl Into<String>) -> Self {
        Self {
            kind,
            status: SuppressionStatus::Accepted,
            justification: justification.into(),
        }
    }

    /// Whether the suppression is in force.
    pub fn in_force(&self) -> bool {
        self.status == SuppressionStatus::Accepted
    }
}

/// A finding a suppression keeps out of the report. A run states how many
/// there were, and SARIF keeps them with their suppression.
#[derive(Debug, Clone, PartialEq)]
pub struct Suppressed {
    pub diagnostic: Diagnostic,
    pub suppression: Suppression,
}
