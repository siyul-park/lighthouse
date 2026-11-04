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
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use lighthouse_model::{Diagnostic, RunScope};
use lighthouse_plugin::{Context, Pending, RuleManifest};
use lighthouse_spec::{Batching, Stdin};
use serde_json::{Map, Value};

use super::{CommandRule, Invocation, chunks};

/// The fewest files worth a process of their own.
const LEAST_BATCH: usize = 32;

/// The findings of the files the batches of a rule held.
type Findings = BTreeMap<PathBuf, Vec<Diagnostic>>;

/// What one run of a program over several files told.
enum Answer {
    /// The findings in each of the files.
    Told(Findings),
    /// It ran and found something it does not attribute to a file.
    Mixed,
    /// It did not run, or what it printed cannot be read.
    Failed,
}

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

    /// Whether the findings for a file of `language` are what the program
    /// finds in that file alone: it is cacheable and `independent`, or bundled
    /// and given the text on stdin, so that no other file, complete or not,
    /// changes them.
    pub(crate) fn alone(&self, scope: RunScope, language: &str) -> bool {
        self.cacheable(scope)
            && self.programs.of(language).is_some_and(|program| {
                program.independent || (self.bundled && program.stdin == Stdin::File)
            })
    }

    /// Whether the findings for a file of `language` also depend on the
    /// project's other files: the program is cacheable but not `independent`,
    /// so it may read what the file names.
    pub(crate) fn siblings(&self, scope: RunScope, language: &str) -> bool {
        self.cacheable(scope)
            && self.programs.of(language).is_some()
            && !self.alone(scope, language)
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
        let (meta, ctx, _) = check;
        let mut answers = BTreeMap::new();
        for (program, files) in self.assigned(ctx) {
            // The files the cache had findings for are not run again.
            let pending = ctx.memo.slot::<Pending>();
            let files: Vec<PathBuf> = files
                .into_iter()
                .filter(|path| pending.wants(&meta.id, path))
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
                let batches = split(chunks(&program.command, &group));
                answers.extend(run.side_by_side(&batches, program.independent));
            }
        }
        answers
    }
}

/// The chunks cut so that there is a batch for each processor, none of fewer
/// than [`LEAST_BATCH`] files: the processors are otherwise idle while one
/// program works through the project.
fn split(chunks: Vec<&[PathBuf]>) -> Vec<&[PathBuf]> {
    let processors = std::thread::available_parallelism().map_or(1, usize::from);
    chunks
        .into_iter()
        .flat_map(|chunk| chunk.chunks(chunk.len().div_ceil(processors).max(LEAST_BATCH)))
        .collect()
}

impl Invocation<'_> {
    /// The findings of `batches`, each run in a thread of its own, joined in
    /// the order of the batches.
    fn side_by_side(&self, batches: &[&[PathBuf]], independent: bool) -> Findings {
        std::thread::scope(|scope| {
            let running: Vec<_> = batches
                .iter()
                .map(|batch| {
                    scope.spawn(move || {
                        let mut found = Findings::new();
                        self.settle(batch, independent, &mut found);
                        found
                    })
                })
                .collect();
            running
                .into_iter()
                .flat_map(|thread| {
                    thread
                        .join()
                        .unwrap_or_else(|e| std::panic::resume_unwind(e))
                })
                .collect::<Findings>()
        })
    }

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
