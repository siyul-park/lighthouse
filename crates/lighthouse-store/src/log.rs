//! The decision log: `.lighthouse/decisions.jsonl`, one record per line,
//! append-only and meant to be committed. Concurrent branches that both
//! appended merge by concatenation (`merge=union`), and the same event read
//! twice is one event.
//!
//! A line is a resource: `apiVersion`, `kind` and `metadata.name` (the id of
//! the record) around a `spec`. This build writes `Verdict` records and reads
//! every kind it knows, skipping the ones it does not, so that a newer build
//! can add kinds without breaking an older one. Lines written before the
//! resource model are one flat event with snake_case keys and no `kind`; they
//! are read as the same event and never rewritten.

use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use lighthouse_model::{Reason, ReviewerKind, Verdict, hash};
use lighthouse_resource::{API_VERSION, Metadata, Resource, Spec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, ReviewEvent, digest};

/// File name of the log inside the store directory.
pub(crate) const FILE: &str = "decisions.jsonl";
/// Length of an event id: the hash of the event without it.
const ID_BYTES: usize = 16;
/// Ids of entries imported from a store that predates the log.
pub(crate) const LEGACY_PREFIX: &str = "legacy-";

/// The spec of the `Verdict` record: a reviewer's judgment about one finding,
/// with the finding frozen as it was seen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VerdictSpec {
    pub fingerprint: String,
    pub rule_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighthouse_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern_fingerprint: Option<String>,
    pub verdict: Verdict,
    pub reason: Reason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_text: Option<String>,
    pub reviewer_kind: ReviewerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_digest: Option<String>,
    /// The finding frozen at review time: evidence, facts, options, severity,
    /// authored severity, when and where it was seen. Its severity is the string recorded
    /// then, so history may say `review`.
    pub snapshot: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub timestamp: String,
}

impl Spec for VerdictSpec {
    const KIND: &'static str = "Verdict";
}

impl From<VerdictSpec> for ReviewEvent {
    fn from(spec: VerdictSpec) -> Self {
        Self {
            id: String::new(),
            fingerprint: spec.fingerprint,
            rule_id: spec.rule_id,
            rule_version: spec.rule_version,
            check_revision: spec.check_revision,
            decision_hash: spec.decision_hash,
            catalog_version: spec.catalog_version,
            lighthouse_version: spec.lighthouse_version,
            pattern_fingerprint: spec.pattern_fingerprint,
            verdict: spec.verdict,
            reason: spec.reason,
            reason_text: spec.reason_text,
            reviewer_kind: spec.reviewer_kind,
            reviewer_id: spec.reviewer_id,
            language: spec.language,
            scope: spec.scope,
            evidence_digest: spec.evidence_digest,
            snapshot: spec.snapshot,
            commit: spec.commit,
            timestamp: spec.timestamp,
        }
    }
}

impl ReviewEvent {
    /// The event as reports show it: the keys of a `Verdict` record in
    /// lowerCamelCase, with the event's `id` and its `label`.
    pub fn to_json(&self) -> Value {
        let mut value =
            serde_json::to_value(VerdictSpec::from(self)).expect("a verdict serializes");
        value["id"] = Value::String(self.id.clone());
        value["label"] = serde_json::json!(self.label());
        value
    }
}

impl From<&ReviewEvent> for VerdictSpec {
    fn from(event: &ReviewEvent) -> Self {
        Self {
            fingerprint: event.fingerprint.clone(),
            rule_id: event.rule_id.clone(),
            rule_version: event.rule_version.clone(),
            check_revision: event.check_revision.clone(),
            decision_hash: event.decision_hash.clone(),
            catalog_version: event.catalog_version.clone(),
            lighthouse_version: event.lighthouse_version.clone(),
            pattern_fingerprint: event.pattern_fingerprint.clone(),
            verdict: event.verdict,
            reason: event.reason,
            reason_text: event.reason_text.clone(),
            reviewer_kind: event.reviewer_kind,
            reviewer_id: event.reviewer_id.clone(),
            language: event.language.clone(),
            scope: event.scope.clone(),
            evidence_digest: event.evidence_digest.clone(),
            snapshot: event.snapshot.clone(),
            commit: event.commit.clone(),
            timestamp: event.timestamp.clone(),
        }
    }
}

/// What a read of the log found.
#[derive(Debug, Default)]
pub(crate) struct Logged {
    pub events: Vec<ReviewEvent>,
    /// Records of a kind or version this build does not know, which a newer
    /// build wrote.
    pub skipped: usize,
}

/// Gives the event the id its content determines.
pub(crate) fn seal(event: &mut ReviewEvent) -> Result<(), Error> {
    event.id = id_of(event)?;
    Ok(())
}

/// The line of an event: a canonical `Verdict` record with keys in sorted
/// order, so that the same event is always the same bytes.
pub(crate) fn line(event: &ReviewEvent) -> Result<String, Error> {
    let record = Resource::new(Metadata::named(&event.id), VerdictSpec::from(event));
    Ok(digest::canonical(&serde_json::to_value(record)?))
}

/// Every event of the log, in file order; none if there is no log. A line
/// that is not valid JSON, a record of a kind this build knows whose content
/// is wrong, or an event whose id does not match its content is an error
/// naming the line: a decision that cannot be read must not be skipped. A
/// record of a kind or version this build does not know is skipped and
/// counted, so that the caller can say so. Fields of a known kind that this
/// build does not know are ignored.
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
        let Some(event) = decode(value).map_err(fail)? else {
            read.skipped += 1;
            continue;
        };
        event
            .verdict
            .validate(event.reason)
            .map_err(|e| Error::Log {
                path: path.display().to_string(),
                line: index + 1,
                reason: e.to_string(),
            })?;
        read.events.push(event);
    }
    Ok(read)
}

/// Appends one event as a line, in a single write, and flushes it to disk
/// before returning.
pub(crate) fn append(path: &Path, event: &ReviewEvent) -> Result<(), Error> {
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
    text.push_str(&line(event)?);
    text.push('\n');
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|source| io(path, source))
}

/// The event a log line holds; `None` for a record this build skips.
fn decode(value: Value) -> Result<Option<ReviewEvent>, String> {
    if value.get("kind").is_none() {
        let event: ReviewEvent = serde_json::from_value(value).map_err(|e| e.to_string())?;
        if !event.id.starts_with(LEGACY_PREFIX)
            && legacy_id_of(&event).map_err(|e| e.to_string())? != event.id
        {
            return Err("the id does not match the entry".to_owned());
        }
        return Ok(Some(event));
    }
    let known = value.get("apiVersion").and_then(Value::as_str) == Some(API_VERSION)
        && value.get("kind").and_then(Value::as_str) == Some(VerdictSpec::KIND);
    if !known {
        return Ok(None);
    }
    // The id covers the spec as written, fields this build does not know
    // included.
    let written = hash::short(
        &digest::canonical(value.get("spec").unwrap_or(&Value::Null)),
        ID_BYTES,
    );
    let record: Resource<VerdictSpec> = serde_json::from_value(value).map_err(|e| e.to_string())?;
    let mut event = ReviewEvent::from(record.spec);
    event.id = record.metadata.name;
    if !event.id.starts_with(LEGACY_PREFIX) && written != event.id {
        return Err("the id does not match the entry".to_owned());
    }
    Ok(Some(event))
}

/// The id of a record: the hash of its spec.
fn id_of(event: &ReviewEvent) -> Result<String, Error> {
    let body = serde_json::to_value(VerdictSpec::from(event))?;
    Ok(hash::short(&digest::canonical(&body), ID_BYTES))
}

/// The id of an entry in the flat shape from before the resource model.
fn legacy_id_of(event: &ReviewEvent) -> Result<String, Error> {
    let mut body = serde_json::to_value(event)?;
    if let Value::Object(map) = &mut body {
        map.remove("id");
    }
    Ok(hash::short(&digest::canonical(&body), ID_BYTES))
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
