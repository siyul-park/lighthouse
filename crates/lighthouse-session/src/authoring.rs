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

use lighthouse_resource::{API_VERSION, STATE_DIR, new_uid};
use lighthouse_spec::{Catalog, local_dir, local_files};
use serde_json::Value;

use crate::{DecisionTest, Result, Session, test_decisions};

const LOCAL_PACK: &str = "local";
/// Held while a decision is authored. Its name matches the `.lighthouse/*.db*`
/// ignore that `init` writes, so it stays out of version control.
const LOCK_FILE: &str = "authoring.db-lock";

/// A decision change that passed its gate and was written.
pub struct Authored {
    pub id: String,
    pub path: PathBuf,
    pub created: bool,
    pub test: DecisionTest,
    /// Whether `lighthouse.toml` lists the `local` plugin, without which the
    /// project's own decisions do not run in `check`.
    pub plugin_listed: bool,
}

/// Adds the local decision the `Decision` document describes. The document is
/// complete: `metadata.name` is its id (`local/<name>`), `spec` carries the
/// examples. A missing `apiVersion` or `metadata.uid` is filled in.
pub fn create_decision(session: Session, mut document: Value) -> Result<Authored> {
    let (id, name) = local_name(&document)?;
    let _lock = lock(&session.root)?;
    if local_files(&session.root)?.is_some_and(|f| f.contains_key(&file_name(&name))) {
        return Err(format!("`{id}` already exists; change it with `decision update`").into());
    }
    complete(&mut document);
    commit(session, &id, &name, document, true)
}

/// Replaces the local decision the `Decision` document names with it, keeping
/// the uid the decision has. A bundled decision is not changed in place: its
/// level and options are the project's `rules`, and different wording or
/// examples are a local decision.
pub fn replace_decision(session: Session, mut document: Value) -> Result<Authored> {
    let id = document_id(&document)?;
    let _lock = lock(&session.root)?;
    let files = local_files(&session.root)?.unwrap_or_default();
    let Some(name) = id.strip_prefix("local/") else {
        if session.catalog()?.decision(&id).is_none() {
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
    commit(session, &id, &name, document, false)
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
    session: Session,
    id: &str,
    name: &str,
    mut doc: Value,
    created: bool,
) -> Result<Authored> {
    ensure_uid(&mut doc);
    let root = session.root.clone();
    let plugin_listed = session.config.lists(LOCAL_PACK);
    let mut files: BTreeMap<String, String> = local_files(&root)?.unwrap_or_default();
    files.insert(file_name(name), serde_norway::to_string(&doc)?);
    let local = Catalog::from_local(files)?;
    let text = canonical(&local, id)?;
    let candidate = session.with_local(Some(local));
    candidate.catalog()?;
    let testable = candidate
        .catalog()?
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
    Ok(Authored {
        id: id.to_owned(),
        path: dir.join(file_name(name)),
        created,
        test,
        plugin_listed,
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
    Ok(Catalog::decision_text(decision)?)
}

fn file_name(name: &str) -> String {
    format!("{name}.yaml")
}

fn parse(text: &str) -> Result<Value> {
    Ok(serde_norway::from_str(text)?)
}
