mod sarif;

use std::{fmt::Write, str::FromStr};

use lighthouse_model::Diagnostic;
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

/// Renders diagnostics in the given order; the output ends with a newline
/// unless it is empty.
pub fn render(format: Format, diagnostics: &[Diagnostic]) -> String {
    match format {
        Format::Text => text(diagnostics),
        Format::Json => json_lines(diagnostics),
        Format::Sarif => sarif::render(diagnostics),
    }
}

fn text(diagnostics: &[Diagnostic]) -> String {
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
    out
}

fn json_lines(diagnostics: &[Diagnostic]) -> String {
    let mut out = String::new();
    for d in diagnostics {
        out.push_str(&serde_json::to_string(d).expect("diagnostic serializes"));
        out.push('\n');
    }
    out
}
