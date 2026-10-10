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

use lighthouse_resource::{API_VERSION, SCHEMA_URL_BASE, resource, to_document};
use lighthouse_spec::{Catalog, DecisionOverrideSpec, local_dir, local_files};
use serde_json::{Map, Value, json};

use crate::{DecisionTest, Result, Session, test_decisions};

const LOCAL_PACK: &str = "local";
/// Held while a decision is authored. Its name matches the `.lighthouse/*.db*`
/// ignore that `init` writes, so it stays out of version control.
const LOCK_FILE: &str = ".lighthouse/authoring.db-lock";

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

/// Adds the local decision `id` (`local/<name>`) with `spec`, the spec of a
/// `Decision` (its check is `type: cel` or absent), and `examples`, appended
/// to the spec's own.
pub fn create_decision(
    session: Session,
    id: &str,
    mut spec: Value,
    examples: Vec<Value>,
) -> Result<Authored> {
    let map = spec
        .as_object_mut()
        .ok_or("`spec` must be an object with the fields of a Decision spec")?;
    if !examples.is_empty() {
        let mut all = match map.remove("examples") {
            Some(Value::Array(own)) => own,
            Some(_) => return Err("`examples` must be a list".into()),
            None => Vec::new(),
        };
        all.extend(examples);
        map.insert("examples".to_owned(), Value::Array(all));
    }
    let name = id
        .strip_prefix("local/")
        .ok_or_else(|| format!("id is `{id}`, a local decision is `local/<name>`"))?
        .to_owned();
    checked_name(&name)?;
    let _lock = lock(&session.root)?;
    if local_files(&session.root)?.is_some_and(|f| f.contains_key(&file_name(&name))) {
        return Err(format!("`{id}` already exists; change it with decision_update").into());
    }
    let doc = json!({
        "apiVersion": API_VERSION,
        "kind": "Decision",
        "metadata": { "name": id },
        "spec": spec,
    });
    commit(session, id, &name, doc, true)
}

/// Applies `patch`, a JSON merge patch (null removes a key), to the spec of
/// the local decision `id`; for any other decision of the catalog it writes or
/// changes the project's override file, whose spec fields are `severity`,
/// `exceptions`, `options`, `languages` and `examples`.
pub fn update_decision(session: Session, id: &str, patch: &Value) -> Result<Authored> {
    if !patch.is_object() {
        return Err("`patch` must be an object".into());
    }
    if patch.get("extends").is_some() {
        return Err("`extends` cannot be changed".into());
    }
    let _lock = lock(&session.root)?;
    let files = local_files(&session.root)?.unwrap_or_default();
    let (name, mut doc, created) = if let Some(name) = id.strip_prefix("local/") {
        checked_name(name)?;
        let text = files
            .get(&file_name(name))
            .ok_or_else(|| format!("`{id}` is not a local decision of this project"))?;
        (name.to_owned(), parse(text)?, false)
    } else {
        if session.catalog()?.decision(id).is_none() {
            return Err(format!("unknown decision `{id}`").into());
        }
        let name = format!("override-{}", id.replace('/', "-"));
        checked_name(&name)?;
        match files.get(&file_name(&name)) {
            Some(text) => (name, parse(text)?, false),
            None => {
                let doc = json!({
                    "apiVersion": API_VERSION,
                    "kind": "DecisionOverride",
                    "metadata": { "name": name },
                    "spec": { "extends": id },
                });
                (name, doc, true)
            }
        }
    };
    let spec = doc
        .get_mut("spec")
        .ok_or_else(|| format!("the file of `{id}` has no `spec`"))?;
    merge(spec, patch);
    commit(session, id, &name, doc, created)
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
    let path = root.join(LOCK_FILE);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = File::options().create(true).append(true).open(&path)?;
    file.lock()?;
    Ok(file)
}

/// Validates and tests the candidate layer, then writes it.
fn commit(session: Session, id: &str, name: &str, doc: Value, created: bool) -> Result<Authored> {
    let root = session.root.clone();
    let plugin_listed = session.config.lists(LOCAL_PACK);
    let mut files: BTreeMap<String, String> = local_files(&root)?.unwrap_or_default();
    files.insert(file_name(name), serde_norway::to_string(&doc)?);
    let local = Catalog::from_local(files)?;
    let text = canonical(&local, id, &doc)?;
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

/// The text written to disk: the typed document, so that keys come in the
/// schema's order and the file starts with the schema comment.
fn canonical(local: &Catalog, id: &str, doc: &Value) -> Result<String> {
    if doc.get("kind").and_then(Value::as_str) == Some("DecisionOverride") {
        let override_doc = resource::<DecisionOverrideSpec>(id, doc)?;
        return Ok(to_document(&override_doc, SCHEMA_URL_BASE));
    }
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

fn merge(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    let Value::Object(map) = target else {
        return;
    };
    for (key, value) in patch {
        if value.is_null() {
            map.remove(key);
        } else {
            merge(map.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}
