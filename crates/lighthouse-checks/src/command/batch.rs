//! Formatters in batches. A bundled program that takes `{files}` and reads
//! nothing from the environment is run once for the files of a batch, by the
//! first file of the run to be judged, and the others take its answer.
//!
//! - A program that is `independent` finds in a file what a run over the file
//!   alone finds, so each file takes the findings the batch has on it.
//! - Of another, a batch that finds nothing says that no file has a finding,
//!   which a run of each file alone would only repeat. One that finds
//!   something answers nothing: the files are run one by one, as a program
//!   that follows what a file names (`rustfmt` and the modules it declares)
//!   is judged by its own run.
//!
//! A batch that fails answers nothing either.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use lighthouse_model::{Diagnostic, RunScope};
use lighthouse_plugin::{Context, RuleManifest};
use lighthouse_spec::{Batching, Stdin};
use serde_json::{Map, Value};

use super::{CommandRule, Invocation, chunks};

/// The findings of the files the batches of a rule held.
type Findings = BTreeMap<PathBuf, Vec<Diagnostic>>;

/// What the batches of a run found in the files they held, by rule.
#[derive(Default)]
pub(super) struct Answers(Mutex<HashMap<String, Arc<OnceLock<Findings>>>>);

/// What a check needs to run a program: its rule, context and options.
type Check<'a> = (&'a RuleManifest, &'a Context<'a>, &'a Map<String, Value>);

impl CommandRule {
    /// Whether the findings for a file come from batches: the programs are
    /// bundled and take their files as arguments, with nothing else to read.
    pub(crate) fn batched(&self, scope: RunScope) -> bool {
        self.bundled
            && scope == RunScope::File
            && self.programs.all().next().is_some()
            && self.programs.all().all(|p| {
                p.env.is_empty() && p.stdin == Stdin::None && p.batching() == Batching::Files
            })
    }

    /// The programs the findings come from, by name.
    pub(crate) fn tools(&self) -> Vec<String> {
        let names: BTreeSet<&String> = self
            .programs
            .all()
            .filter_map(|p| p.command.first())
            .collect();
        names.into_iter().cloned().collect()
    }

    /// What a batch of the run found in `file`, when one held it and ran.
    pub(super) fn answered(&self, check: Check, file: &Path) -> Option<Vec<Diagnostic>> {
        let (meta, ctx, _) = check;
        // A fix run stands texts in place of files, which a batch cannot read.
        if !self.batched(meta.scope) || !ctx.workspace.overlays.is_empty() {
            return None;
        }
        let cell = {
            let slot = ctx.memo.slot::<Answers>();
            let mut by_rule = slot.0.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(by_rule.entry(meta.id.clone()).or_default())
        };
        cell.get_or_init(|| self.answers(check)).get(file).cloned()
    }

    /// The findings in each file of the project that a run of its program
    /// over a group of files answered for. Files the rule is not applied to
    /// are left out.
    fn answers(&self, check: Check) -> Findings {
        let (_, ctx, _) = check;
        let mut answers = BTreeMap::new();
        for (program, files) in self.assigned(ctx) {
            let files: Vec<PathBuf> = files
                .into_iter()
                .filter(|path| {
                    ctx.project
                        .file(path)
                        .is_some_and(|file| !ctx.applies.excludes(ctx.project, file))
                })
                .collect();
            let Ok(run) = self.invocation(program, check, &ctx.workspace.root) else {
                continue;
            };
            let Ok(groups) = run.grouped(files) else {
                continue;
            };
            for group in groups {
                for chunk in chunks(&program.command, &group) {
                    if let Some(found) = run.answer(chunk, program.independent) {
                        answers.extend(found);
                    }
                }
            }
        }
        answers
    }
}

impl Invocation<'_> {
    /// The findings in each of `files` from one run over them, when it ran
    /// and they can be told apart: those of a program that is `independent`
    /// are on the file they name, and none of another's.
    fn answer(&self, files: &[PathBuf], independent: bool) -> Option<Findings> {
        let command = self.arguments(files).ok()?;
        let printed = self.invoke(&command, &[]).ok()?;
        let found = self.collect(&printed, None).ok()?;
        if !independent && !found.is_empty() {
            return None;
        }
        let mut by_file: Findings = files.iter().map(|f| (f.clone(), Vec::new())).collect();
        for diagnostic in found {
            by_file.get_mut(&diagnostic.file)?.push(diagnostic);
        }
        Some(by_file)
    }
}
