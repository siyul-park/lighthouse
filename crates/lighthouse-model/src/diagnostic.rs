use std::{fmt, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::Span;

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

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub rule_id: String,
    pub severity: Severity,
    pub message: String,
    pub file: PathBuf,
    pub span: Span,
    pub fingerprint: Fingerprint,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub evidence: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Diagnostic {
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
            evidence: Value::Null,
            fix: None,
        }
    }
}
