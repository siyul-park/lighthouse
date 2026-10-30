//! Authoring project-local decisions. A change is written to
//! `.lighthouse/decisions` only after the candidate layer loads, its checks
//! compile and every example passes the whole engine; a rejected change leaves
//! the project exactly as it was.

use std::{
    collections::BTreeMap,
    fs,
    fs::File,
    path::{Path, PathBuf},
};

use lighthouse_model::{API_VERSION, Attribution, STATE_DIR, new_uid};
use lighthouse_spec::{Catalog, local_dir, local_files};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::{
    DecisionTest, Result, Session,
    signal::{DUPLICATE_THRESHOLD, Duplicate, candidates, record_proposal, settle_derived},
    test_decisions,
};

/// Most possible duplicates a reply names.
const DUPLICATES: usize = 3;

pub(crate) const LOCAL_PACK: &str = "local";
/// Held while a decision is authored. Its name matches the `.lighthouse/*.db*`
/// ignore that `init` writes, so it stays out of version control.
const LOCK_FILE: &str = "authoring.db-lock";

/// A decision change that passed its gate and was written: the
/// `DecisionReport` kind.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Authored {
    pub id: String,
    pub path: PathBuf,
    pub created: bool,
    /// What the gate ran before the decision was written.
    #[serde(rename = "tested")]
    pub test: DecisionTest,
    /// What to do next.
    pub note: String,
    /// Existing decisions the new one may repeat; the decision is written
    /// anyway, because a person decides.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub duplicates: Vec<Duplicate>,
    /// The signals now recorded or linked, by id.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<String>,
    /// Signals the document names that the store does not hold.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown_signals: Vec<String>,
}

/// Adds the local decision the `Decision` document describes. The document is
/// complete: `metadata.name` is its id (`local/<name>`), `spec` carries the
/// examples. A missing `apiVersion` or `metadata.uid` is filled in, and a
/// missing `spec.status` is `proposed`: only a person accepts a decision.
/// Decisions that may repeat it are named in the result, and the signals it
/// derives from become recorded.
pub fn create_decision(
    session: &Session,
    mut document: Value,
    by: &Attribution,
) -> Result<Authored> {
    let (id, name) = local_name(&document)?;
    let _lock = lock(&session.root)?;
    if local_files(&session.root)?.is_some_and(|f| f.contains_key(&file_name(&name))) {
        return Err(format!("`{id}` already exists; change it with `decision update`").into());
    }
    complete(&mut document);
    if let Some(spec) = document.get_mut("spec").and_then(Value::as_object_mut) {
        spec.entry("status")
            .or_insert_with(|| Value::String("proposed".to_owned()));
    }
    let query = format!(
        "{} {}",
        document
            .pointer("/spec/title")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        document
            .pointer("/spec/requirement")
            .and_then(Value::as_str)
            .unwrap_or_default()
    );
    let duplicates = candidates(session, &query, Some(&id), DUPLICATE_THRESHOLD, DUPLICATES)?;
    commit(session, &id, &name, document, Some(duplicates), by)
}

/// Replaces the local decision the `Decision` document names with it, keeping
/// the uid the decision has. A bundled decision is not changed in place: its
/// level and options are the project's `rules`, and different wording or
/// examples are a local decision.
pub fn replace_decision(
    session: &Session,
    mut document: Value,
    by: &Attribution,
) -> Result<Authored> {
    let id = document_id(&document)?;
    let _lock = lock(&session.root)?;
    let files = local_files(&session.root)?.unwrap_or_default();
    let Some(name) = id.strip_prefix("local/") else {
        if session.catalog.decision(&id).is_none() {
            return Err(format!("unknown decision `{id}`").into());
        }
        return Err(format!(
            "`{id}` is not a local decision of this project: set its level and options in `rules` of lighthouse.toml, or write a local decision with `decision create`"
        )
        .into());
    };
    checked_name(name)?;
    let text = files
        .get(&file_name(name))
        .ok_or_else(|| format!("`{id}` is not a local decision of this project"))?;
    let uid = parse(text)?
        .pointer("/metadata/uid")
        .cloned()
        .filter(Value::is_string);
    complete(&mut document);
    if let (Some(uid), Some(metadata)) = (uid, document.get_mut("metadata")) {
        metadata["uid"] = uid;
    }
    let name = name.to_owned();
    commit(session, &id, &name, document, None, by)
}

/// The id of a document and the name of the file it is kept in.
fn local_name(document: &Value) -> Result<(String, String)> {
    let id = document_id(document)?;
    let name = id
        .strip_prefix("local/")
        .ok_or_else(|| format!("id is `{id}`, a local decision is `local/<name>`"))?
        .to_owned();
    checked_name(&name)?;
    Ok((id, name))
}

/// `metadata.name` of a `Decision` document.
fn document_id(document: &Value) -> Result<String> {
    if document.get("kind").and_then(Value::as_str) != Some("Decision") {
        return Err("the document must be a `Decision` (`kind: Decision`)".into());
    }
    document
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "the document needs `metadata.name`, the decision id".into())
}

/// Fills in what a document may leave out: the API version and a uid.
fn complete(document: &mut Value) {
    if let Some(map) = document.as_object_mut() {
        map.entry("apiVersion")
            .or_insert_with(|| Value::String(API_VERSION.to_owned()));
    }
    ensure_uid(document);
}

/// Refuses a decision name that could name a file outside `.lighthouse/decisions`.
fn checked_name(name: &str) -> Result<()> {
    if Catalog::local_name_ok(name) {
        return Ok(());
    }
    Err(format!(
        "`{name}` is not a valid decision name: lowercase letters, digits, `.`, `_` and `-`, starting with a letter or digit, without `..`"
    )
    .into())
}

/// Serializes authoring in a project, across threads and processes.
fn lock(root: &Path) -> Result<File> {
    let path = root.join(STATE_DIR).join(LOCK_FILE);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = File::options().create(true).append(true).open(&path)?;
    file.lock()?;
    Ok(file)
}

/// Validates and tests the candidate layer, then writes it.
fn commit(
    session: &Session,
    id: &str,
    name: &str,
    mut doc: Value,
    duplicates: Option<Vec<Duplicate>>,
    by: &Attribution,
) -> Result<Authored> {
    let created = duplicates.is_some();
    let duplicates = duplicates.unwrap_or_default();
    ensure_uid(&mut doc);
    let root = session.root.clone();
    let mut files: BTreeMap<String, String> = local_files(&root)?.unwrap_or_default();
    files.insert(file_name(name), serde_norway::to_string(&doc)?);
    let local = Catalog::from_local(files, session.comp.cel())?;
    let text = canonical(&local, id)?;
    let candidate = session.with_local(Some(local))?;
    let testable = candidate
        .catalog
        .decision(id)
        .is_some_and(|d| d.automated());
    let test = if testable {
        test_decisions(&candidate, &[id.to_owned()], None)?
    } else {
        DecisionTest {
            decisions: 0,
            languages: 0,
            runs: 0,
            failures: Vec::new(),
            skipped: Vec::new(),
        }
    };
    if !test.failures.is_empty() {
        return Err(format!(
            "`{id}` was not written, its examples fail:\n{}",
            test.failures.join("\n")
        )
        .into());
    }
    let dir = local_dir(&root);
    Catalog::write_local(&dir, name, &text)?;
    let mut note = "enable the decision with `extends = [\"local/recommended\"]` or a [rules] entry if it is not on yet".to_owned();
    let uid = doc
        .pointer("/metadata/uid")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let settled = settle_derived(session, &doc, &uid, created, by)?;
    if created {
        record_proposal(session, &uid, id, &duplicates)?;
    }
    if !duplicates.is_empty() {
        let names: Vec<&str> = duplicates.iter().map(|d| d.id.as_str()).collect();
        note = format!(
            "possible duplicate of {}: a person decides whether to keep both; {note}",
            names.join(", ")
        );
    }
    Ok(Authored {
        id: id.to_owned(),
        path: dir.join(file_name(name)),
        created,
        test,
        note,
        duplicates,
        signals: settled.signals,
        unknown_signals: settled.unknown,
    })
}

/// Gives a decision written before uids existed the one it keeps from now on.
fn ensure_uid(doc: &mut Value) {
    if let Some(metadata) = doc.get_mut("metadata").and_then(Value::as_object_mut) {
        metadata
            .entry("uid")
            .or_insert_with(|| Value::String(new_uid()));
    }
}

/// The text written to disk: the typed document, so that keys come in the
/// schema's order and the file starts with the schema comment.
fn canonical(local: &Catalog, id: &str) -> Result<String> {
    let decision = local
        .decision(id)
        .ok_or_else(|| format!("`{id}` is not in the candidate layer"))?;
    Ok(Catalog::decision_text(decision))
}

fn file_name(name: &str) -> String {
    format!("{name}.yaml")
}

fn parse(text: &str) -> Result<Value> {
    Ok(serde_norway::from_str(text)?)
}
