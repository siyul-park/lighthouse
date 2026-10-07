//! Authoring project-local rules. A change is written to
//! `.lighthouse/rules` only after the candidate layer loads, its rules
//! compile and every example passes the whole engine; a rejected change
//! leaves the project exactly as it was.

use std::{
    collections::BTreeMap,
    fs,
    fs::File,
    path::{Path, PathBuf},
};

use lighthouse_declarative::{local_dir, local_files};
use lighthouse_spec::Catalog;
use serde_json::{Map, Value};

use crate::{Result, RuleTest, Session, test_rules};

const LOCAL_PACK: &str = "local";
/// Held while a rule is authored. Its name matches the `.lighthouse/*.db*`
/// ignore that `init` writes, so it stays out of version control.
const LOCK_FILE: &str = ".lighthouse/authoring.db-lock";

/// A rule change that passed its gate and was written.
pub struct Authored {
    pub id: String,
    pub path: PathBuf,
    pub created: bool,
    pub test: RuleTest,
    /// Whether `lighthouse.toml` lists the `local` plugin, without which the
    /// project's own rules do not run in `check`.
    pub plugin_listed: bool,
}

/// Adds the local rule described by `pattern` (all pattern fields but
/// `implementation`, id `local/<name>`), its declarative `rule` and
/// `examples` (appended to the pattern's own).
pub fn create_rule(
    session: Session,
    mut pattern: Value,
    rule: Option<Value>,
    examples: Vec<Value>,
) -> Result<Authored> {
    let doc = pattern
        .as_object_mut()
        .ok_or("`pattern` must be an object with the fields of a pattern")?;
    doc.remove("implementation");
    if let Some(rule) = rule {
        doc.insert("rule".to_owned(), rule);
    }
    if !examples.is_empty() {
        let mut all = match doc.remove("examples") {
            Some(Value::Array(own)) => own,
            Some(_) => return Err("`examples` must be a list".into()),
            None => Vec::new(),
        };
        all.extend(examples);
        doc.insert("examples".to_owned(), Value::Array(all));
    }
    let id = doc
        .get("id")
        .and_then(Value::as_str)
        .ok_or("the pattern needs an `id`")?
        .to_owned();
    let name = id
        .strip_prefix("local/")
        .ok_or_else(|| format!("id is `{id}`, a local rule is `local/<name>`"))?
        .to_owned();
    checked_name(&name)?;
    let _lock = lock(&session.root)?;
    if local_files(&session.root)?.is_some_and(|f| f.contains_key(&file_name(&name))) {
        return Err(format!("`{id}` already exists; change it with rule_update").into());
    }
    commit(session, &id, &name, pattern, true)
}

/// Applies `patch`, a JSON merge patch (null removes a key), to the local
/// rule `id`; for any other pattern of the catalog it writes or changes the
/// project's overlay file, whose fields are `severity`, `exceptions`,
/// `options`, `tuning` and `examples`.
pub fn update_rule(session: Session, id: &str, patch: &Value) -> Result<Authored> {
    if !patch.is_object() {
        return Err("`patch` must be an object".into());
    }
    let _lock = lock(&session.root)?;
    let files = local_files(&session.root)?.unwrap_or_default();
    let (name, mut doc, created) = if let Some(name) = id.strip_prefix("local/") {
        checked_name(name)?;
        let text = files
            .get(&file_name(name))
            .ok_or_else(|| format!("`{id}` is not a local rule of this project"))?;
        (name.to_owned(), parse(text)?, false)
    } else {
        if session.catalog()?.pattern(id).is_none() {
            return Err(format!("unknown pattern `{id}`").into());
        }
        let name = format!("override-{}", id.replace('/', "-"));
        checked_name(&name)?;
        match files.get(&file_name(&name)) {
            Some(text) => (name, parse(text)?, false),
            None => (name, serde_json::json!({ "extends": id }), true),
        }
    };
    for key in ["id", "extends"] {
        if patch.get(key).is_some() {
            return Err(format!("`{key}` cannot be changed").into());
        }
    }
    merge(&mut doc, patch);
    commit(session, id, &name, doc, created)
}

/// Refuses a rule name that could name a file outside `.lighthouse/rules`.
fn checked_name(name: &str) -> Result<()> {
    if Catalog::local_name_ok(name) {
        return Ok(());
    }
    Err(format!(
        "`{name}` is not a valid rule name: lowercase letters, digits, `.`, `_` and `-`, starting with a letter or digit, without `..`"
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
    let text = serde_norway::to_string(&doc)?;
    files.insert(file_name(name), text.clone());
    let local = Catalog::from_local(files)?;
    let text = canonical(&local, id, &doc, text)?;
    let candidate = session.with_local(Some(local));
    candidate.catalog()?;
    let testable = candidate
        .catalog()?
        .pattern(id)
        .is_some_and(|p| p.implementation.is_some());
    let test = if testable {
        test_rules(&candidate, &[id.to_owned()], None)?
    } else {
        RuleTest {
            patterns: 0,
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

/// A new local rule is written in the loader's own layout; an overlay file
/// as given.
fn canonical(local: &Catalog, id: &str, doc: &Value, text: String) -> Result<String> {
    let Some(pattern) = local.pattern(id).filter(|_| doc.get("extends").is_none()) else {
        return Ok(text);
    };
    let rule = doc
        .get("rule")
        .ok_or("a local rule needs a `rule:` section")?;
    Ok(Catalog::local_rule_text(pattern, rule)?)
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
