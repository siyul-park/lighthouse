//! `lighthouse spec validate` and `lighthouse schema`: the commands that work
//! on spec documents and on the shapes the commands print.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use lighthouse_cli::surface::tools;
use lighthouse_session::{Session, config_file, local_dir, schema_file, schemas, validate_paths};
use serde_json::Value;

use crate::terminal::Result;

/// Validates the documents under `paths`; the project's configuration and
/// local decisions when there are none. Returns 1 when anything is wrong.
pub fn validate(config: Option<&Path>, paths: &[PathBuf], examples: bool) -> Result<u8> {
    let root = lighthouse_session::project_root(config)?;
    // The documents under check may be the very ones that do not load.
    let session = match Session::load_or_default(config) {
        Ok(session) => session,
        Err(_) => Session::bare(root.clone())?,
    };
    let paths = if paths.is_empty() {
        let mut paths: Vec<PathBuf> = config_file(&root).into_iter().collect();
        paths.extend(Some(local_dir(&root)).filter(|d| d.is_dir()));
        paths
    } else {
        paths.to_vec()
    };
    let report = validate_paths(&session, &paths, examples)?;
    for problem in &report.problems {
        println!("{}: {}", problem.path, problem.message);
    }
    println!(
        "validated {} document(s) in {} file(s): {} problem(s)",
        report.documents,
        report.files,
        report.problems.len()
    );
    Ok(u8::from(!report.problems.is_empty()))
}

/// Prints one schema, lists the kinds, or with `write` writes every schema
/// into a directory.
pub fn schema(kind: Option<&str>, write: Option<&Path>) -> Result<u8> {
    if let Some(dir) = write {
        fs::create_dir_all(dir)?;
        for (kind, schema) in all_schemas() {
            let mut text = serde_json::to_string_pretty(&schema)?;
            text.push('\n');
            fs::write(dir.join(schema_file(&kind)), text)?;
        }
        return Ok(0);
    }
    match kind {
        Some(name) => {
            let found = all_schemas()
                .into_iter()
                .find(|(kind, _)| kind.eq_ignore_ascii_case(name) || schema_file(kind) == name);
            let (_, schema) = found.ok_or_else(|| {
                let kinds: Vec<String> = all_schemas().into_keys().collect();
                format!(
                    "unknown kind `{name}` (expected one of {})",
                    kinds.join(", ")
                )
            })?;
            println!("{}", serde_json::to_string_pretty(&schema)?);
        }
        None => {
            for kind in all_schemas().into_keys() {
                println!("{kind}\t{}", schema_file(&kind));
            }
        }
    }
    Ok(0)
}

/// Every schema the tool serves, by kind: the spec documents and log records,
/// then what the commands print.
fn all_schemas() -> BTreeMap<String, Value> {
    let documents = schemas()
        .into_values()
        .map(|descriptor| (descriptor.kind.to_owned(), descriptor.schema));
    let outputs = tools::kinds()
        .into_iter()
        .map(|(kind, schema)| (kind.to_owned(), schema));
    documents.chain(outputs).collect()
}
