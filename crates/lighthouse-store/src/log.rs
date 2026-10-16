//! The decision log: `.lighthouse/decisions.jsonl`, one record per line,
//! append-only and meant to be committed. Concurrent branches that both
//! appended merge by concatenation (`merge=union`), and the same record read
//! twice is one record.
//!
//! A line is a resource: `apiVersion`, `kind` and `metadata.name` (the id of
//! the record, the hash of its spec) around a `spec`. The log holds three
//! kinds: `Judgment`, a label on a subject (see [`JudgmentSpec`]),
//! `Suppression`, a finding that is right and is left in place on purpose (see
//! [`SuppressionSpec`]), and `Revision`, a change to how a decision is judged
//! that someone approved (see [`RevisionSpec`]). A line of any other kind, or whose id does not match
//! its spec, is an error that names the file and the line: a decision that
//! cannot be read must not be skipped. The one exception is a last line an
//! interrupted write cut short.
//!
//! The log is the only source of truth. Records are written here first, in one
//! append, and the cache is derived from them; nothing is ever written here
//! from the cache.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use lighthouse_model::{
    Attribution, Judgment, Label, SuppressionKind, SuppressionStatus, canonical, hash,
};
use lighthouse_resource::{API_VERSION, Metadata, Resource, Spec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{Error, Records};

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
    // The key stays `decisionName`: the bytes of a record are what its id hashes.
    #[serde(rename = "decisionName")]
    pub decision: String,
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

/// How far a revision moves a decision. Binding or retraining a model is
/// `minor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    Major,
    Minor,
    Patch,
}

/// The spec of the `Revision` record: a change to how a decision is judged,
/// approved by whoever it is attributed to. One that carries a `model` binds
/// that model to the decision before every other binding; one that carries
/// none unbinds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevisionSpec {
    /// The uid of the decision, else its name.
    pub decision: String,
    /// The id of the revision this one follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meaning_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_revision: Option<String>,
    pub change: Change,
    /// The evaluation that justified it: metrics, labelled-set size and hash.
    #[serde(default)]
    pub evaluation: Value,
    /// `prov:wasAttributedTo`: who approved it.
    pub approval: Attribution,
    /// The model it binds, `<provider>/<model>` (a learned one is
    /// `local/learned@<artifact hash>`). A revision is a full snapshot: absent
    /// means unbound, so a writer that does not change the model copies its
    /// parent's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `prov:generatedAtTime`.
    pub generated_at_time: String,
}

impl Spec for RevisionSpec {
    const KIND: &'static str = "Revision";
}

/// A revision as the log holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct RevisionEvent {
    pub id: String,
    pub spec: RevisionSpec,
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

/// A judgment as the log holds it, with the suppressions that go with it.
#[derive(Debug, Clone, PartialEq)]
pub struct JudgmentEvent {
    pub id: String,
    pub spec: JudgmentSpec,
    pub suppressions: Vec<SuppressionEvent>,
}

/// A suppression as the log holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct SuppressionEvent {
    pub id: String,
    pub spec: SuppressionSpec,
}

impl JudgmentEvent {
    /// The judgment of `spec` with the id its content determines.
    pub(crate) fn new(spec: JudgmentSpec) -> Result<Self, Error> {
        Ok(Self {
            id: id_of(&spec)?,
            spec,
            suppressions: Vec::new(),
        })
    }

    /// What the judgment teaches a model about its decision.
    pub fn label(&self) -> Label {
        Label::new(
            self.spec.judgment,
            self.suppressions
                .iter()
                .any(|s| s.spec.status == SuppressionStatus::Accepted),
        )
    }

    /// The finding's evidence when it was judged.
    pub fn evidence(&self) -> &Value {
        &self.spec.snapshot["evidence"]
    }

    /// The judgment as reports show it: the keys of a `Judgment` record in
    /// lowerCamelCase, with the record's `id`, its `label` and its
    /// `suppressions`.
    pub fn to_json(&self) -> Value {
        let mut value = serde_json::to_value(&self.spec).expect("a judgment serializes");
        value["id"] = Value::String(self.id.clone());
        value["label"] = json!(self.label());
        if !self.suppressions.is_empty() {
            let suppressions: Vec<Value> = self
                .suppressions
                .iter()
                .map(SuppressionEvent::to_json)
                .collect();
            value["suppressions"] = Value::Array(suppressions);
        }
        value
    }
}

impl SuppressionEvent {
    /// The external suppression that goes with `judgment`, written by whoever
    /// wrote the judgment, at the same time.
    pub(crate) fn external(judgment: &JudgmentEvent, justification: &str) -> Result<Self, Error> {
        let spec = SuppressionSpec {
            judgment: judgment.id.clone(),
            fingerprint: judgment.spec.fingerprint.clone(),
            kind: SuppressionKind::External,
            status: SuppressionStatus::Accepted,
            justification: justification.to_owned(),
            was_attributed_to: judgment.spec.was_attributed_to.clone(),
            generated_at_time: judgment.spec.generated_at_time.clone(),
        };
        Ok(Self {
            id: id_of(&spec)?,
            spec,
        })
    }

    /// The suppression as its judgment shows it: the record without the
    /// judgment and the finding it goes with.
    fn to_json(&self) -> Value {
        let mut value = serde_json::to_value(&self.spec).expect("a suppression serializes");
        if let Some(object) = value.as_object_mut() {
            object.remove("judgment");
            object.remove("fingerprint");
        }
        value["id"] = Value::String(self.id.clone());
        value
    }
}

/// What a log line holds.
enum Record {
    Judgment(Box<JudgmentEvent>),
    Suppression(Box<SuppressionEvent>),
    Revision(Box<RevisionEvent>),
}

/// The id of a spec: the hash of its canonical form.
pub(crate) fn id_of<S: Serialize>(spec: &S) -> Result<String, Error> {
    Ok(hash::short_digest(&serde_json::to_value(spec)?, ID_BYTES))
}

/// The line of a record: a canonical resource with keys in sorted order, so
/// that the same record is always the same bytes.
pub(crate) fn line<S: Spec + Serialize>(id: &str, spec: S) -> Result<String, Error> {
    let record = Resource::new(Metadata::named(id), spec);
    Ok(canonical(&serde_json::to_value(record)?))
}

/// Every record of the log, in file order; none if there is no log. A line
/// that is not valid JSON, a record of a kind this build does not write, or
/// one whose id does not match its content is an error naming the line. The
/// exception is a torn last line: one that does not end with a newline and is
/// not JSON is what an interrupted write leaves, and is skipped with a notice.
/// A suppression that goes with no judgment of its finding is skipped with a
/// notice too, and the others are attached to their judgment, by
/// `generatedAtTime` then id. A record read twice is one record.
pub(crate) fn read(path: &Path) -> Result<Records, Error> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Records::default()),
        Err(source) => return Err(io(path, source)),
    };
    let mut read = Records::default();
    let mut suppressions = Vec::new();
    let mut seen = BTreeSet::new();
    let torn_from = (!text.ends_with('\n')).then(|| text.lines().count());
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
        let value: Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(_) if torn_from == Some(index + 1) => {
                read.notices.push(format!(
                    "{}: line {} is cut short and was skipped; the next judgment replaces it",
                    path.display(),
                    index + 1
                ));
                continue;
            }
            Err(e) => return Err(fail(e.to_string())),
        };
        match decode(value).map_err(fail)? {
            Record::Judgment(event) if seen.insert(event.id.clone()) => read.judgments.push(*event),
            Record::Suppression(event) if seen.insert(event.id.clone()) => {
                suppressions.push(*event)
            }
            Record::Revision(event) if seen.insert(event.id.clone()) => read.revisions.push(*event),
            _ => {}
        }
    }
    suppressions.sort_by(|a, b| {
        (&a.spec.generated_at_time, &a.id).cmp(&(&b.spec.generated_at_time, &b.id))
    });
    let positions: BTreeMap<String, usize> = read
        .judgments
        .iter()
        .enumerate()
        .map(|(position, judgment)| (judgment.id.clone(), position))
        .collect();
    for suppression in suppressions {
        let judgment = positions
            .get(&suppression.spec.judgment)
            .map(|&position| &mut read.judgments[position])
            .filter(|judgment| judgment.spec.fingerprint == suppression.spec.fingerprint);
        match judgment {
            Some(judgment) => judgment.suppressions.push(suppression),
            None => read.notices.push(format!(
                "{}: the suppression {} goes with no judgment of {} and was ignored",
                path.display(),
                suppression.id,
                suppression.spec.fingerprint
            )),
        }
    }
    Ok(read)
}

/// Appends `lines` in a single write and flushes it to disk before
/// returning, so a judgment and its suppression are in the log together or
/// not at all. A last line an interrupted write cut short is cut off first.
pub(crate) fn append(path: &Path, lines: &[String]) -> Result<(), Error> {
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
        let torn = cut_torn_line(&mut file).map_err(|source| io(path, source))?;
        if !torn {
            text.push('\n');
        }
    }
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
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
    let kinds = [
        JudgmentSpec::KIND,
        SuppressionSpec::KIND,
        RevisionSpec::KIND,
    ];
    if !kinds.contains(&kind) {
        return Err(format!(
            "`{kind}` is not a kind of the decision log (expected {})",
            kinds.join(", ")
        ));
    }
    // The id covers the spec as written, fields this build does not know
    // included.
    let written = hash::short_digest(value.get("spec").unwrap_or(&Value::Null), ID_BYTES);
    let name = value
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if written != name {
        return Err("the id does not match the entry".to_owned());
    }
    Ok(match kind {
        JudgmentSpec::KIND => Record::Judgment(Box::new(JudgmentEvent {
            id: name,
            spec: spec_of(value)?,
            suppressions: Vec::new(),
        })),
        RevisionSpec::KIND => Record::Revision(Box::new(RevisionEvent {
            id: name,
            spec: spec_of(value)?,
        })),
        _ => Record::Suppression(Box::new(SuppressionEvent {
            id: name,
            spec: spec_of(value)?,
        })),
    })
}

/// The spec of a record line.
fn spec_of<S: Spec + DeserializeOwned>(value: Value) -> Result<S, String> {
    let record: Resource<S> = serde_json::from_value(value).map_err(|e| e.to_string())?;
    Ok(record.spec)
}

/// Truncates the file after its last newline when the last line is not JSON.
/// Returns whether it did.
fn cut_torn_line(file: &mut fs::File) -> std::io::Result<bool> {
    let mut all = String::new();
    file.seek(SeekFrom::Start(0))?;
    file.read_to_string(&mut all)?;
    let start = all.rfind('\n').map_or(0, |i| i + 1);
    if serde_json::from_str::<Value>(&all[start..]).is_ok() {
        return Ok(false);
    }
    file.set_len(start as u64)?;
    file.seek(SeekFrom::End(0))?;
    Ok(true)
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
