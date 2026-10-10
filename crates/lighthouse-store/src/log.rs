//! The decision log: `.lighthouse/decisions.jsonl`, one record per line,
//! append-only and meant to be committed. Concurrent branches that both
//! appended merge by concatenation (`merge=union`), and the same record read
//! twice is one record.
//!
//! A line is a resource: `apiVersion`, `kind` and `metadata.name` (the id of
//! the record, the hash of its spec) around a `spec`. The log holds two
//! kinds: `Judgment`, a label on a subject (see [`JudgmentSpec`]), and
//! `Suppression`, a finding that is right and is left in place on purpose (see
//! [`SuppressionSpec`]). A line of any other kind, or whose id does not match
//! its spec, is an error that names the file and the line: a decision that
//! cannot be read must not be skipped.

use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use lighthouse_model::{Attribution, Judgment, SuppressionKind, SuppressionStatus, hash};
use lighthouse_resource::{API_VERSION, Metadata, Resource, Spec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, JudgmentEvent, SuppressionEvent, digest};

/// File name of the log inside the store directory.
pub(crate) const FILE: &str = "decisions.jsonl";
/// Length of a record id: the hash of its spec.
const ID_BYTES: usize = 16;

/// The spec of the `Judgment` record: a label on one subject (here a finding),
/// with the finding frozen as it was seen. It is a recorded label, not ground
/// truth, and it belongs to the decision's `meaningVersion`: it expires when
/// that moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JudgmentSpec {
    /// The finding the judgment is about.
    pub fingerprint: String,
    /// The name the decision had when the judgment was given, kept to be read;
    /// `decisionUid` is what identifies it.
    pub decision_name: String,
    /// The uid of the decision: its identity across renames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meaning_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighthouse_version: Option<String>,
    pub judgment: Judgment,
    /// Why, written for the next reader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `prov:wasAttributedTo`: a `Person` or a `SoftwareAgent`.
    pub was_attributed_to: Attribution,
    /// `prov:generatedAtTime`.
    pub generated_at_time: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_digest: Option<String>,
    /// The finding frozen at judgment time: evidence, facts, options,
    /// severities, when and where it was seen.
    pub snapshot: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

impl Spec for JudgmentSpec {
    const KIND: &'static str = "Judgment";
}

/// The spec of the `Suppression` record: the `fail` judgment `judgment` is
/// right and is left in place on purpose, in the words of SARIF.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SuppressionSpec {
    /// The id of the judgment it goes with.
    pub judgment: String,
    /// The finding the judgment is about.
    pub fingerprint: String,
    pub kind: SuppressionKind,
    /// `accepted` unless said otherwise.
    #[serde(default)]
    pub status: SuppressionStatus,
    pub justification: String,
    pub was_attributed_to: Attribution,
    pub generated_at_time: String,
}

impl Spec for SuppressionSpec {
    const KIND: &'static str = "Suppression";
}

impl From<&JudgmentEvent> for JudgmentSpec {
    fn from(event: &JudgmentEvent) -> Self {
        Self {
            fingerprint: event.fingerprint.clone(),
            decision_name: event.decision_name.clone(),
            decision_uid: event.decision_uid.clone(),
            meaning_version: event.meaning_version.clone(),
            check_revision: event.check_revision.clone(),
            decision_hash: event.decision_hash.clone(),
            catalog_version: event.catalog_version.clone(),
            lighthouse_version: event.lighthouse_version.clone(),
            judgment: event.judgment,
            reason: event.reason.clone(),
            was_attributed_to: event.was_attributed_to.clone(),
            generated_at_time: event.generated_at_time.clone(),
            language: event.language.clone(),
            scope: event.scope.clone(),
            evidence_digest: event.evidence_digest.clone(),
            snapshot: event.snapshot.clone(),
            commit: event.commit.clone(),
        }
    }
}

impl JudgmentEvent {
    fn from_spec(id: String, spec: JudgmentSpec) -> Self {
        Self {
            id,
            fingerprint: spec.fingerprint,
            decision_name: spec.decision_name,
            decision_uid: spec.decision_uid,
            meaning_version: spec.meaning_version,
            check_revision: spec.check_revision,
            decision_hash: spec.decision_hash,
            catalog_version: spec.catalog_version,
            lighthouse_version: spec.lighthouse_version,
            judgment: spec.judgment,
            reason: spec.reason,
            was_attributed_to: spec.was_attributed_to,
            generated_at_time: spec.generated_at_time,
            language: spec.language,
            scope: spec.scope,
            evidence_digest: spec.evidence_digest,
            snapshot: spec.snapshot,
            commit: spec.commit,
            suppressions: Vec::new(),
        }
    }

    /// The judgment as reports show it: the keys of a `Judgment` record in
    /// lowerCamelCase, with the record's `id`, its `label` and its
    /// `suppressions`.
    pub fn to_json(&self) -> Value {
        let mut value =
            serde_json::to_value(JudgmentSpec::from(self)).expect("a judgment serializes");
        value["id"] = Value::String(self.id.clone());
        value["label"] = serde_json::json!(self.label());
        if !self.suppressions.is_empty() {
            let suppressions: Vec<Value> = self
                .suppressions
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "id": s.id,
                        "kind": s.kind,
                        "status": s.status,
                        "justification": s.justification,
                        "wasAttributedTo": s.was_attributed_to,
                        "generatedAtTime": s.generated_at_time,
                    })
                })
                .collect();
            value["suppressions"] = Value::Array(suppressions);
        }
        value
    }
}

/// A suppression read from the log with what it goes with.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoggedSuppression {
    pub judgment: String,
    pub fingerprint: String,
    pub event: SuppressionEvent,
}

impl LoggedSuppression {
    fn from_spec(id: String, spec: SuppressionSpec) -> Self {
        Self {
            judgment: spec.judgment,
            fingerprint: spec.fingerprint,
            event: SuppressionEvent {
                id,
                kind: spec.kind,
                status: spec.status,
                justification: spec.justification,
                was_attributed_to: spec.was_attributed_to,
                generated_at_time: spec.generated_at_time,
            },
        }
    }

    pub(crate) fn spec(&self) -> SuppressionSpec {
        SuppressionSpec {
            judgment: self.judgment.clone(),
            fingerprint: self.fingerprint.clone(),
            kind: self.event.kind,
            status: self.event.status,
            justification: self.event.justification.clone(),
            was_attributed_to: self.event.was_attributed_to.clone(),
            generated_at_time: self.event.generated_at_time.clone(),
        }
    }
}

/// What a read of the log found.
#[derive(Debug, Default)]
pub(crate) struct Logged {
    pub judgments: Vec<JudgmentEvent>,
    pub suppressions: Vec<LoggedSuppression>,
}

/// What a log line holds.
enum Record {
    Judgment(Box<JudgmentEvent>),
    Suppression(Box<LoggedSuppression>),
}

/// Gives the judgment the id its content determines.
pub(crate) fn seal(event: &mut JudgmentEvent) -> Result<(), Error> {
    event.id = id_of(&JudgmentSpec::from(&*event))?;
    Ok(())
}

/// The id of a spec: the hash of its canonical form.
pub(crate) fn id_of<S: Serialize>(spec: &S) -> Result<String, Error> {
    Ok(hash::short(
        &digest::canonical(&serde_json::to_value(spec)?),
        ID_BYTES,
    ))
}

/// The line of a record: a canonical resource with keys in sorted order, so
/// that the same record is always the same bytes.
pub(crate) fn line<S: Spec + Serialize>(id: &str, spec: S) -> Result<String, Error> {
    let record = Resource::new(Metadata::named(id), spec);
    Ok(digest::canonical(&serde_json::to_value(record)?))
}

/// Every record of the log, in file order; none if there is no log. A line
/// that is not valid JSON, a record of a kind this build does not write, or
/// one whose id does not match its content is an error naming the line.
pub(crate) fn read(path: &Path) -> Result<Logged, Error> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Logged::default()),
        Err(source) => return Err(io(path, source)),
    };
    let mut read = Logged::default();
    for (index, text) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let fail = |reason: String| Error::Log {
            path: path.display().to_string(),
            line: index + 1,
            reason,
        };
        let value: Value = serde_json::from_str(text).map_err(|e| fail(e.to_string()))?;
        match decode(value).map_err(fail)? {
            Record::Judgment(event) => read.judgments.push(*event),
            Record::Suppression(suppression) => read.suppressions.push(*suppression),
        }
    }
    Ok(read)
}

/// Appends one line, in a single write, and flushes it to disk before
/// returning.
pub(crate) fn append(path: &Path, line: &str) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|source| io(dir, source))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
        .map_err(|source| io(path, source))?;
    let mut text = String::new();
    if ends_without_newline(&mut file).map_err(|source| io(path, source))? {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|source| io(path, source))
}

/// The record a log line holds.
fn decode(value: Value) -> Result<Record, String> {
    let Some(kind) = value.get("kind").and_then(Value::as_str) else {
        return Err("not a resource: it has no `kind`".to_owned());
    };
    if value.get("apiVersion").and_then(Value::as_str) != Some(API_VERSION) {
        return Err(format!("`apiVersion` is not `{API_VERSION}`"));
    }
    if kind != JudgmentSpec::KIND && kind != SuppressionSpec::KIND {
        return Err(format!(
            "`{kind}` is not a kind of the decision log (expected {} or {})",
            JudgmentSpec::KIND,
            SuppressionSpec::KIND
        ));
    }
    // The id covers the spec as written, fields this build does not know
    // included.
    let written = hash::short(
        &digest::canonical(value.get("spec").unwrap_or(&Value::Null)),
        ID_BYTES,
    );
    let name = value
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if written != name {
        return Err("the id does not match the entry".to_owned());
    }
    if kind == JudgmentSpec::KIND {
        let record: Resource<JudgmentSpec> =
            serde_json::from_value(value).map_err(|e| e.to_string())?;
        Ok(Record::Judgment(Box::new(JudgmentEvent::from_spec(
            name,
            record.spec,
        ))))
    } else {
        let record: Resource<SuppressionSpec> =
            serde_json::from_value(value).map_err(|e| e.to_string())?;
        Ok(Record::Suppression(Box::new(LoggedSuppression::from_spec(
            name,
            record.spec,
        ))))
    }
}

fn ends_without_newline(file: &mut fs::File) -> std::io::Result<bool> {
    if file.metadata()?.len() == 0 {
        return Ok(false);
    }
    let mut last = [0u8; 1];
    file.seek(SeekFrom::End(-1))?;
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
}

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}
