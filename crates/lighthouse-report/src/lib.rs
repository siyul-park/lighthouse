mod sarif;

use std::{fmt::Write, str::FromStr};

use lighthouse_model::{Diagnostic, Incomplete, Severity};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
    Sarif,
}

#[derive(Debug, Error)]
#[error("unknown format `{0}` (expected text, json or sarif)")]
pub struct UnknownFormat(String);

impl FromStr for Format {
    type Err = UnknownFormat;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "sarif" => Ok(Self::Sarif),
            _ => Err(UnknownFormat(s.to_owned())),
        }
    }
}

/// Renders diagnostics in the given order, then what the analysis could not
/// cover; the output ends with a newline unless it is empty. Text output ends
/// with a `summary:` line of counts whenever there is anything to report. An incomplete
/// analysis is stated in every format: text prints `incomplete` lines, JSON
/// prints `{"incomplete": {...}}` lines and SARIF marks its invocation as
/// unsuccessful with a tool notification per entry.
pub fn render(format: Format, diagnostics: &[Diagnostic], incomplete: &[Incomplete]) -> String {
    match format {
        Format::Text => text(diagnostics, incomplete),
        Format::Json => json_lines(diagnostics, incomplete),
        Format::Sarif => sarif::render(diagnostics, incomplete),
    }
}

fn text(diagnostics: &[Diagnostic], incomplete: &[Incomplete]) -> String {
    let mut out = String::new();
    for d in diagnostics {
        let _ = writeln!(
            out,
            "{}:{}:{}: {} {}: {}",
            d.file.display(),
            d.span.start.line,
            d.span.start.col,
            d.severity,
            d.rule_id,
            d.message,
        );
    }
    for item in incomplete {
        match &item.path {
            Some(path) => {
                let _ = writeln!(out, "{}: incomplete: {}", path.display(), item.reason);
            }
            None => {
                let _ = writeln!(out, "incomplete: {}", item.reason);
            }
        }
    }
    if !diagnostics.is_empty() || !incomplete.is_empty() {
        let count = |s: Severity| diagnostics.iter().filter(|d| d.severity == s).count();
        let _ = writeln!(
            out,
            "summary: {} error, {} warn, {} review, {} incomplete",
            count(Severity::Error),
            count(Severity::Warn),
            count(Severity::Review),
            incomplete.len()
        );
    }
    out
}

fn json_lines(diagnostics: &[Diagnostic], incomplete: &[Incomplete]) -> String {
    let mut out = String::new();
    for d in diagnostics {
        out.push_str(&serde_json::to_string(d).expect("diagnostic serializes"));
        out.push('\n');
    }
    for item in incomplete {
        let line = serde_json::json!({ "incomplete": item });
        out.push_str(&line.to_string());
        out.push('\n');
    }
    out
}
