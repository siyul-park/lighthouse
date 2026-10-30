//! The one path a rewrite of the project takes, whoever makes it. A program
//! changes files in a scratch copy of the project, a repository of one commit
//! (see [`scratch`]); what it changed is collected against the texts it started
//! from, held to the scope of the fix (see [`accept`]) and, if it stays within
//! it, is the proposal the orchestrator applies in memory and keeps only if the
//! checks accept it. A command fix and an agent that edits differ in the
//! program they run and in how the proposal is judged, not in this path. What
//! the program says it did is never taken for the result.

mod scope;
mod scratch;
#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use lighthouse_model::{Span, ucm::File};
use lighthouse_protocol::hash;
use lighthouse_spec::FixScope;

use scope::{first_outside, reached};
#[cfg(test)]
pub(in crate::fix) use scratch::run_verify_in;
pub(in crate::fix) use scratch::{Change, Changes, Scratch, run_verify};

use super::model::Class;
use crate::{
    Overlays, Run,
    cache::{Digest, digest_of},
};

/// What a program works over, which is also all it may read.
pub(in crate::fix) struct World<'a> {
    pub(in crate::fix) root: &'a Path,
    /// The check the findings come from.
    pub(in crate::fix) outcome: &'a Run,
    /// The texts that stand in for the files of the project: those of the
    /// rounds before, and every edit already accepted in this pass.
    pub(in crate::fix) texts: &'a Overlays,
    /// The files of the project as they stand now, which an answer is kept
    /// under: an edit made over other texts is another answer. Found when
    /// asked for.
    digest: OnceLock<Result<Digest, String>>,
}

impl<'a> World<'a> {
    pub(in crate::fix) fn new(root: &'a Path, outcome: &'a Run, texts: &'a Overlays) -> Self {
        Self {
            root,
            outcome,
            texts,
            digest: OnceLock::new(),
        }
    }

    /// The text `path` has before the program runs.
    pub(in crate::fix) fn text(&self, path: &Path) -> Option<String> {
        match self.texts.get(path) {
            Some(text) => Some(text.clone()),
            None => fs::read_to_string(self.root.join(path)).ok(),
        }
    }

    /// The digest of the files of the project as they stand.
    pub(in crate::fix) fn digest(&self) -> Result<&Digest, String> {
        self.digest
            .get_or_init(|| {
                let mut files: Vec<(&Path, String)> = self
                    .outcome
                    .project
                    .files
                    .iter()
                    .map(|file| (file.path.as_path(), self.stand(file)))
                    .collect();
                files.sort_unstable();
                digest_of(&files)
                    .map_err(|e| format!("the files of the project could not be digested: {e}"))
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The digest of the file as it stands: the text that stands in for it,
    /// else the hash of the file on disk.
    fn stand(&self, file: &File) -> String {
        self.texts
            .get(&file.path)
            .map_or_else(|| file.hash.clone(), hash::sha256)
    }
}

/// How far the change of one attempt may go: the scope of its fix, the file
/// its findings are in and, for the scope `symbol`, the extent of each.
pub(in crate::fix) struct Bounds<'a> {
    /// Who made the change, as the reasons say it: `the agent`, `the command`.
    pub(in crate::fix) who: &'a str,
    pub(in crate::fix) scope: FixScope,
    pub(in crate::fix) file: &'a Path,
    pub(in crate::fix) extents: &'a [Span],
}

/// The files `scratch` differs from the project in, with the digest of each
/// that was not created as the program found it.
pub(in crate::fix) fn collect(
    scratch: &Scratch,
    world: &World<'_>,
) -> Result<(Changes, BTreeMap<PathBuf, String>), String> {
    let changes = scratch
        .changes(world.root, world.texts)
        .map_err(|e| format!("the scratch copy of the project could not be read: {e}"))?;
    let mut bases = BTreeMap::new();
    for (path, change) in &changes {
        if *change == Change::Created {
            continue;
        }
        let found = world.text(path).ok_or_else(|| {
            format!(
                "`{}`, which the program changed, cannot be read",
                path.display()
            )
        })?;
        bases.insert(path.clone(), hash::sha256(&found));
    }
    Ok((changes, bases))
}

/// The new texts of the files the program changed when all of it is within
/// the bounds, else why the work is rejected whole.
pub(in crate::fix) fn accept(
    changes: Changes,
    bounds: &Bounds<'_>,
    world: &World<'_>,
) -> Result<BTreeMap<PathBuf, String>, (Class, String)> {
    let reach = (bounds.scope == FixScope::References).then(|| {
        let texts: BTreeMap<PathBuf, (String, String)> = changes
            .iter()
            .filter_map(|(path, change)| match change {
                Change::Text(new) => Some((path.clone(), (world.text(path)?, new.clone()))),
                _ => None,
            })
            .collect();
        reached(&world.outcome.project, bounds.file, &texts)
    });
    if let Some(offence) = changes
        .iter()
        .find_map(|(path, change)| offence(path, change, (bounds, reach.as_ref()), world))
    {
        return Err((Class::OutOfScope, offence));
    }
    let texts: BTreeMap<PathBuf, String> = changes
        .into_iter()
        .filter_map(|(path, change)| match change {
            Change::Text(text) => Some((path, text)),
            _ => None,
        })
        .collect();
    if texts.is_empty() {
        return Err((Class::Unchanged, format!("{} changed no file", bounds.who)));
    }
    Ok(texts)
}

/// What is wrong with the program having done `change` to `path`, if
/// anything: it may change the indexed text files of the project under the
/// scope `files`, those that `reach` holds under `references`, only the file of
/// the findings under the scope `file`, and under the scope `symbol` only the
/// extents of the findings in their file.
fn offence(
    path: &Path,
    change: &Change,
    (bounds, reach): (&Bounds<'_>, Option<&BTreeSet<PathBuf>>),
    world: &World<'_>,
) -> Option<String> {
    let shown = path.display();
    let who = bounds.who;
    let Change::Text(text) = change else {
        return Some(match change {
            Change::Created => format!("{who} created `{shown}`"),
            Change::Deleted => format!("{who} deleted `{shown}`"),
            _ => format!("{who} changed `{shown}`, which is not text"),
        });
    };
    if world.outcome.project.file(path).is_none() {
        return Some(format!(
            "{who} changed `{shown}`, which Lighthouse does not index"
        ));
    }
    let scope = bounds.scope;
    if scope == FixScope::Files {
        return None;
    }
    if let Some(reach) = reach {
        return (!reach.contains(path)).then(|| {
            format!(
                "{who} changed `{shown}`, which refers to nothing changed in the file of the findings or in the files it reaches (scope `references`)"
            )
        });
    }
    if path != bounds.file {
        return Some(format!(
            "{who} changed `{shown}`, which none of the findings is in (scope `{scope}`)"
        ));
    }
    if scope == FixScope::File {
        return None;
    }
    let Some(old) = world.text(path) else {
        return Some(format!("`{shown}` cannot be read"));
    };
    let line = first_outside(&old, text, bounds.extents)?;
    Some(format!(
        "{who} changed line {line} of `{shown}`, outside the symbol of the findings (scope `symbol`)"
    ))
}
