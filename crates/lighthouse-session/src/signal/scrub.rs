//! Hides secrets in text before it is stored. The patterns are the common
//! ones of secret scanners such as gitleaks, kept in this one table: a token
//! of a known provider, a private key, a bearer token, a credential in a URL,
//! and an assignment to a name that says it is a secret.

use std::{borrow::Cow, sync::LazyLock};

use regex::Regex;

/// What a secret is replaced with.
pub const REDACTED: &str = "[REDACTED]";

/// `(name, pattern, replacement)`: a replacement may keep the part before the
/// secret (`$1`).
const PATTERNS: [(&str, &str, &str); 14] = [
    (
        "private-key",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z ]*PRIVATE KEY-----|\z)",
        REDACTED,
    ),
    (
        "aws-access-key",
        r"\b(?:AKIA|ASIA|AGPA|AIDA|AROA|ANPA|ANVA)[A-Z0-9]{16}\b",
        REDACTED,
    ),
    (
        "aws-secret",
        r#"(?i)(aws[\w.-]{0,20}(?:secret|key)[\w.-]{0,10}\s*[:=]\s*['"]?)[A-Za-z0-9/+=]{40}\b"#,
        "${1}[REDACTED]",
    ),
    (
        "github-token",
        r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{22,})\b",
        REDACTED,
    ),
    ("slack-token", r"\bxox[abprs]-[A-Za-z0-9-]{10,}", REDACTED),
    (
        "stripe-key",
        r"\b[sr]k_(?:live|test)_[A-Za-z0-9]{16,}\b",
        REDACTED,
    ),
    ("google-api-key", r"\bAIza[0-9A-Za-z_-]{35}\b", REDACTED),
    (
        "llm-api-key",
        r"\bsk-(?:ant-|proj-|or-)?[A-Za-z0-9_-]{20,}\b",
        REDACTED,
    ),
    (
        "jwt",
        r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b",
        REDACTED,
    ),
    (
        "bearer",
        r"(?i)(\bbearer\s+)[A-Za-z0-9._~+/=-]{20,}",
        "${1}[REDACTED]",
    ),
    (
        "url-credentials",
        r"(://[^/\s:@]+:)[^/\s@]+@",
        "${1}[REDACTED]@",
    ),
    (
        "basic-auth",
        r"(?i)(\bbasic\s+)[A-Za-z0-9+/]{16,}={0,2}",
        "${1}[REDACTED]",
    ),
    (
        "secret-assignment",
        r#"(?i)(\b[\w.-]*(?:api[_-]?key|secret|token|passwd|password|pwd|credential)[\w.-]*\s*[:=]\s*['"]?)[^\s'",;]{8,}"#,
        "${1}[REDACTED]",
    ),
    (
        "env-assignment",
        r#"(?i)(\b[A-Z][A-Z0-9_]*(?:KEY|SECRET|TOKEN|PASSWORD)\s*=\s*['"]?)[^\s'",;]{8,}"#,
        "${1}[REDACTED]",
    ),
];

static COMPILED: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    PATTERNS
        .iter()
        .map(|(name, pattern, with)| {
            (
                Regex::new(pattern).unwrap_or_else(|e| panic!("secret pattern `{name}`: {e}")),
                *with,
            )
        })
        .collect()
});

/// `text` with every secret the patterns know hidden.
pub fn scrub(text: &str) -> String {
    COMPILED
        .iter()
        .fold(Cow::Borrowed(text), |text, (pattern, with)| {
            match pattern.replace_all(&text, *with) {
                Cow::Borrowed(_) => text,
                Cow::Owned(changed) => Cow::Owned(changed),
            }
        })
        .into_owned()
}
