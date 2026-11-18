//! Findings as the engine keeps them between runs (in its graph and its
//! reports): the finding as it is reported, and with it the subject a rule
//! selected, which no outward form of a finding shows. A finding read back is
//! the one that was kept.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Diagnostic;

/// A finding with its subject, for keeping.
#[derive(Serialize)]
struct Kept<'a> {
    #[serde(flatten)]
    diagnostic: &'a Diagnostic,
    #[serde(skip_serializing_if = "Option::is_none")]
    subject: &'a Option<String>,
}

/// A finding with its subject, as read back.
#[derive(Deserialize)]
struct Read {
    #[serde(flatten)]
    diagnostic: Diagnostic,
    #[serde(default)]
    subject: Option<String>,
}

impl From<Read> for Diagnostic {
    fn from(read: Read) -> Self {
        Self {
            subject: read.subject,
            ..read.diagnostic
        }
    }
}

/// Keeps `findings` as JSON, with their subjects.
pub fn to_string(findings: &[Diagnostic]) -> Result<String, serde_json::Error> {
    serde_json::to_string(&findings.iter().map(kept).collect::<Vec<_>>())
}

/// The findings kept as `text` by [`to_string`].
pub fn from_str(text: &str) -> Result<Vec<Diagnostic>, serde_json::Error> {
    let read: Vec<Read> = serde_json::from_str(text)?;
    Ok(read.into_iter().map(Diagnostic::from).collect())
}

/// For `#[serde(with = "lighthouse_model::kept")]` on a list of findings.
pub fn serialize<S: Serializer>(findings: &[Diagnostic], to: S) -> Result<S::Ok, S::Error> {
    to.collect_seq(findings.iter().map(kept))
}

/// For `#[serde(with = "lighthouse_model::kept")]` on a list of findings.
pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<Vec<Diagnostic>, D::Error> {
    let read = Vec::<Read>::deserialize(from)?;
    Ok(read.into_iter().map(Diagnostic::from).collect())
}

fn kept(diagnostic: &Diagnostic) -> Kept<'_> {
    Kept {
        diagnostic,
        subject: &diagnostic.subject,
    }
}
