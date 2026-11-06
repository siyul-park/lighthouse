//! Formatters in batches. A bundled program that takes `{files}` and reads
//! nothing from the environment is run once for the files of a batch, by the
//! first file of the run to be judged, and the others take its answer.
//!
//! - A program that is `independent` finds in a file what a run over the file
//!   alone finds, so each file takes the findings the batch has on it.
//! - Of another, a batch that finds nothing says that no file has a finding,
//!   which a run of each file alone would only repeat. One that finds
//!   something cannot tell whose it is: it is halved until the batches that
//!   find nothing are answered and the files with findings are left to be
//!   run one by one, as a program that follows what a file names (`rustfmt`
//!   and the modules it declares) is judged by its own run.
//!
//! A batch that fails answers nothing either. The batches of a group run
//! side by side, and their answers are joined in the order of the files.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use lighthouse_model::{Diagnostic, RunScope};
use lighthouse_plugin::{BATCHED, Batched, Context, Error as PluginError, RuleManifest};
use lighthouse_spec::{Batching, Stdin};
use serde_json::{Map, Value};

use super::{CommandRule, Invocation, chunks};

/// The findings of the files the batches of a rule held.
type Findings = BTreeMap<PathBuf, Vec<Diagnostic>>;

/// What one run of a program over several files told.
enum Answer {
    /// The findings in each file.
    Told(Findings),
    /// It ran and found something it does not attribute to a file.
    Mixed,
    /// It did not run, or what it printed cannot be read.
    Failed,
}

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

    /// Whether the findings for a file may be stored: no program keeps a
    /// secret in its environment, and the decision either is bundled or says
    /// what its programs read, `independent` (the file alone) or `follows`
    /// (what it names too).
    pub(crate) fn cacheable(&self, scope: RunScope) -> bool {
        scope == RunScope::File
            && self.programs.all().next().is_some()
            && self.programs.all().all(|p| p.env.is_empty())
            && (self.bundled || self.programs.all().all(|p| p.independent || p.follows))
    }

    /// Whether the findings for a file of `language` also depend on which
    /// files the project has: the program is cacheable but not `independent`,
    /// so it may read what the file names.
    pub(super) fn siblings(&self, scope: RunScope, language: &str) -> bool {
        self.cacheable(scope)
            && self.programs.of(language).is_some()
            && !self.alone(scope, language)
    }

    /// Whether the findings for a file of `language` are what the program
    /// finds in that file alone: it is cacheable and `independent`, or bundled
    /// and given the text on stdin, so that no other file, complete or not,
    /// changes them.
    fn alone(&self, scope: RunScope, language: &str) -> bool {
        self.cacheable(scope)
            && self.programs.of(language).is_some_and(|program| {
                program.independent || (self.bundled && program.stdin == Stdin::File)
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

    /// What the batch of the file found in it, when the batch ran and told
    /// it: the answer is a node of the graph, shared by the files of the
    /// batch.
    pub(super) fn answered(
        &self,
        (meta, ctx, _): Check,
        file: &Path,
    ) -> Result<Option<Vec<Diagnostic>>, PluginError> {
        if !self.batched(meta.scope) || ctx.batch.files.is_empty() {
            return Ok(None);
        }
        let key = serde_json::to_string(&(&meta.id, &ctx.batch.variant, &ctx.batch.files))
            .map_err(|e| PluginError::Failed(e.to_string()))?;
        let batched = ctx
            .queries
            .pull(BATCHED, &key)?
            .downcast::<Batched>()
            .map_err(|_| PluginError::Failed(format!("`{BATCHED}` has an unexpected value")))?;
        let name = file.to_string_lossy();
        let Some((_, json)) = batched.answered.iter().find(|(path, _)| *path == name) else {
            return Ok(None);
        };
        batched
            .notices
            .iter()
            .for_each(|notice| ctx.notices.push(notice.clone()));
        serde_json::from_str(json)
            .map(Some)
            .map_err(|e| PluginError::Failed(e.to_string()))
    }

    /// The findings in each of `files` that a run of the program over them
    /// told, for the files of the language it is the program of. Files the
    /// rule is not applied to are left out by the host.
    pub(super) fn answers(&self, check: Check, files: &[PathBuf]) -> Result<Findings, PluginError> {
        let (meta, ctx, _) = check;
        let mut by: BTreeMap<Option<&str>, Vec<PathBuf>> = BTreeMap::new();
        for path in files.iter().filter(|path| self.selects(path)) {
            let Some(file) = ctx.model.file(path) else {
                continue;
            };
            if self.programs.of(&file.lang).is_some() {
                let key = self
                    .programs
                    .is_language(&file.lang)
                    .then_some(file.lang.as_str());
                by.entry(key).or_default().push(path.clone());
            }
        }
        let held = self.root_for(ctx, None)?;
        let root = held
            .as_ref()
            .map_or_else(|| ctx.workspace.root.clone(), |dir| dir.path().to_owned());
        let mut answers = Findings::new();
        for (key, files) in by {
            let Some(program) = key.map_or(Some(self.programs.own()), |l| self.programs.of(l))
            else {
                continue;
            };
            let Ok(run) = self.invocation(program, check, &root) else {
                continue;
            };
            let Ok(groups) = run.grouped(files) else {
                continue;
            };
            for group in groups {
                for chunk in chunks(&program.command, &group) {
                    run.settle(chunk, program.independent, &mut answers);
                }
            }
        }
        let _ = meta;
        Ok(answers)
    }
}

impl Invocation<'_> {
    /// The findings of `files` from runs over them, in `answers`. A run that
    /// found something a program that is not `independent` does not attribute
    /// is repeated over each half of the files, so that the clean half is
    /// answered at once and only the files with findings run alone; a file
    /// left unanswered is checked by itself.
    fn settle(&self, files: &[PathBuf], independent: bool, answers: &mut Findings) {
        match self.answer(files, independent) {
            Answer::Told(found) => answers.extend(found),
            Answer::Mixed if files.len() > 1 => {
                let (first, second) = files.split_at(files.len() / 2);
                self.settle(first, independent, answers);
                self.settle(second, independent, answers);
            }
            Answer::Mixed | Answer::Failed => {}
        }
    }

    /// The findings in each of `files` from one run over them, when it ran
    /// and they can be told apart: those of a program that is `independent`
    /// are on the file they name, and none of another's.
    fn answer(&self, files: &[PathBuf], independent: bool) -> Answer {
        let Ok(command) = self.arguments(files) else {
            return Answer::Failed;
        };
        let Ok(printed) = self.invoke(&command, &[]) else {
            return Answer::Failed;
        };
        let Ok(found) = self.collect(&printed, None) else {
            return Answer::Failed;
        };
        if !independent && !found.is_empty() {
            return Answer::Mixed;
        }
        let mut by_file: Findings = files.iter().map(|f| (f.clone(), Vec::new())).collect();
        for diagnostic in found {
            let Some(own) = by_file.get_mut(&diagnostic.file) else {
                return Answer::Failed;
            };
            own.push(diagnostic);
        }
        Answer::Told(by_file)
    }
}
