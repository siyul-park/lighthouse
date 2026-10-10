//! `lighthouse spec validate`, `lighthouse spec migrate` and `lighthouse
//! schema`: the commands that work on spec documents themselves.

use std::{
    fs,
    path::{Path, PathBuf},
};

use lighthouse_session::{
    Session, config_file, migrate_paths, schema_file, schema_of, schemas, validate_paths,
};

use crate::Result;

/// Where a project keeps its own decisions, for the default of both commands.
const LOCAL_DECISIONS: &str = ".lighthouse/decisions";
const LOCAL_RULES: &str = ".lighthouse/rules";

/// Validates the documents under `paths`; the project's configuration and
/// local decisions when there are none. Returns 1 when anything is wrong.
pub fn validate(paths: &[PathBuf], examples: bool) -> Result<u8> {
    let root = lighthouse_session::project_root()?;
    // The documents under check may be the very ones that do not load.
    let session = match Session::load_or_default(None) {
        Ok(session) => session,
        Err(_) => Session::bare(root.clone())?,
    };
    let paths = if paths.is_empty() {
        defaults(&root, &[LOCAL_DECISIONS])
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

/// Migrates the documents under `paths`; the project's configuration and
/// `.lighthouse/rules` when there are none.
pub fn migrate(paths: &[PathBuf], dry_run: bool) -> Result<u8> {
    let paths = if paths.is_empty() {
        defaults(
            &lighthouse_session::project_root()?,
            &[LOCAL_RULES, LOCAL_DECISIONS],
        )
    } else {
        paths.to_vec()
    };
    let migrated = migrate_paths(&paths, dry_run)?;
    let verb = if dry_run { "would write" } else { "wrote" };
    for path in &migrated.written {
        println!("{verb} {}", path.display());
    }
    let verb = if dry_run { "would remove" } else { "removed" };
    for path in &migrated.removed {
        println!("{verb} {}", path.display());
    }
    for warning in &migrated.warnings {
        eprintln!("warning: {warning}");
    }
    println!(
        "{} file(s) written, {} removed, {} document(s) already migrated",
        migrated.written.len(),
        migrated.removed.len(),
        migrated.unchanged
    );
    Ok(0)
}

/// Prints one schema, lists the kinds, or with `write` writes every schema
/// into a directory.
pub fn schema(kind: Option<&str>, write: Option<&Path>) -> Result<u8> {
    if let Some(dir) = write {
        fs::create_dir_all(dir)?;
        for descriptor in schemas().into_values() {
            let mut text = serde_json::to_string_pretty(&descriptor.schema)?;
            text.push('\n');
            fs::write(dir.join(schema_file(descriptor.kind)), text)?;
        }
        return Ok(0);
    }
    match kind {
        Some(kind) => {
            let descriptor = schema_of(kind).ok_or_else(|| {
                let kinds: Vec<&str> = schemas().keys().copied().collect();
                format!(
                    "unknown kind `{kind}` (expected one of {})",
                    kinds.join(", ")
                )
            })?;
            println!("{}", serde_json::to_string_pretty(&descriptor.schema)?);
        }
        None => {
            for descriptor in schemas().into_values() {
                println!("{}\t{}", descriptor.kind, schema_file(descriptor.kind));
            }
        }
    }
    Ok(0)
}

/// The project's configuration file and the given directories, as far as they
/// exist.
fn defaults(root: &Path, dirs: &[&str]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = config_file(root).into_iter().collect();
    paths.extend(dirs.iter().map(|d| root.join(d)).filter(|d| d.is_dir()));
    paths
}
