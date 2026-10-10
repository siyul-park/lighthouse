//! Folding the overrides of a project into its `rules`: each override file
//! under `.lighthouse` is read, its level and options are written to the
//! configuration of the project that owns the directory, and the file is
//! removed.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use lighthouse_resource::Format;
use lighthouse_spec::FILE_NAMES;
use serde_norway::Value;

use super::{Action, LOCAL_ROOT, Plan, render_config, retired, several_documents};
use crate::Result;

/// Schedules an override document for folding into the rules of the project
/// whose `.lighthouse` directory holds it. An override that says more than a
/// project's rules can, or that no project owns, is kept with a note.
pub(super) fn plan(plan: &mut Plan, path: &Path, doc: &Value) -> std::result::Result<(), String> {
    let json = serde_json::to_value(doc).map_err(|e| e.to_string())?;
    match retired::Fold::of(&json) {
        Ok(fold) if project_file(path).is_some() => plan.folds.push((path.to_owned(), fold)),
        Ok(_) => plan.kept.push(format!(
            "{}: kept, no lighthouse.toml beside its .lighthouse directory to take the rule",
            path.display()
        )),
        Err(why) => plan.kept.push(format!("{}: kept, {why}", path.display())),
    }
    Ok(())
}

/// Writes the scheduled folds into the configuration of their projects and
/// removes the override files.
pub(super) fn apply(plan: &mut Plan) -> Result<()> {
    let mut by_project: BTreeMap<PathBuf, Vec<(PathBuf, retired::Fold)>> = BTreeMap::new();
    for (file, fold) in std::mem::take(&mut plan.folds) {
        let Some(config) = project_file(&file) else {
            continue;
        };
        by_project.entry(config).or_default().push((file, fold));
    }
    for (config, folds) in by_project {
        let format = Format::of_path(&config).unwrap_or(Format::Toml);
        let label = config.display().to_string();
        let text = match plan.actions.get(&config) {
            Some(Action::Write(text)) => text.clone(),
            _ => fs::read_to_string(&config)?,
        };
        let mut docs = lighthouse_resource::documents(format, &label, &text)?;
        if docs.len() != 1 {
            return Err(several_documents(&label, docs.len()).into());
        }
        let mut project = docs.remove(0);
        for (file, fold) in &folds {
            fold.apply(&mut project)
                .map_err(|e| format!("{}: {e}", file.display()))?;
            plan.actions.insert(file.clone(), Action::Remove);
        }
        let text = render_config(format, &config, &project)?;
        plan.actions.insert(config, Action::Write(text));
    }
    Ok(())
}

/// The configuration of the project whose `.lighthouse` directory holds `path`.
fn project_file(path: &Path) -> Option<PathBuf> {
    let absolute = path
        .canonicalize()
        .ok()
        .or_else(|| std::env::current_dir().ok().map(|dir| dir.join(path)))?;
    let root = absolute
        .ancestors()
        .find(|dir| dir.file_name().is_some_and(|n| n == LOCAL_ROOT))?
        .parent()?
        .to_owned();
    lighthouse_resource::file_in(&root, &FILE_NAMES)
}
