use std::{fmt, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::Span;

/// How a finding is acted on; serialized and parsed as the lowercase variant name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Mechanical violation.
    Error,
    /// Heuristic signal.
    Warn,
    /// Delegated to an agent or human with evidence.
    Review,
    Info,
}

/// The text given to [`Severity::from_str`] names no severity.
#[derive(Debug, Error)]
#[error("unknown severity `{0}` (expected error, warn, review or info)")]
pub struct UnknownSeverity(pub String);

impl FromStr for Severity {
    type Err = UnknownSeverity;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "error" => Ok(Self::Error),
            "warn" => Ok(Self::Warn),
            "review" => Ok(Self::Review),
            "info" => Ok(Self::Info),
            _ => Err(UnknownSeverity(s.to_owned())),
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Review => "review",
            Self::Info => "info",
        })
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
        let mut hash = Sha256::new();
        for part in [rule_id, symbol_path.as_str()] {
            hash.update(part);
            hash.update([0]);
        }
        hash.update(snippet.split_whitespace().collect::<Vec<_>>().join(" "));
        Self(hex(&hash.finalize()))
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
        let mut hash = Sha256::new();
        hash.update(&self.0);
        hash.update(n.to_le_bytes());
        Self(hex(&hash.finalize()))
    }

    /// Tells apart findings that share a fingerprint by something stable about
    /// where they are, such as the symbol that encloses them.
    pub fn discriminate(&self, discriminator: &str) -> Self {
        let mut hash = Sha256::new();
        hash.update(&self.0);
        hash.update([0xff]);
        hash.update(discriminator);
        Self(hex(&hash.finalize()))
    }

    /// The opaque hex string; equal fingerprints identify the same finding.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One finding: where a rule found a violation and how to tell it apart from
/// the same finding in another run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        }
    }
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
