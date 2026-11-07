//! The fix by a command: the program of a decision's `fix` runs for one
//! finding, through the rewrite path it shares with an agent that edits (see
//! [`super::rewrite`]). A program that reads the file on stdin and prints its
//! new text (`output: stdout`) is a filter and needs no copy of the project:
//! its fix is a node of the graph (see [`super::node`]). Every other program
//! runs in a scratch copy of the project and leaves its change there, which
//! no file-sized node can stand for. A change within the scope of the fix is
//! the proposal; a run that changed nothing declines the finding, with what
//! the program said as the reason.

use lighthouse_model::ucm::Model;
use std::time::Instant;

use lighthouse_model::{Diagnostic, Safety};
use lighthouse_spec::{FixScope, Output, Point, Program};

use super::{
    CommandFix, FixBinding,
    lower::Sources,
    model::{Class, choose_extent},
    node::Made,
    program::{Failure, Launch, NO_FACTS, edits_of, filter, filtered, is_filter, run, unchanged},
    propose::{Proposal, Proposer},
    rewrite::{Bounds, Changes, Scratch, World, accept, collect},
};
use crate::{Outcome, Overlays};

impl Proposer<'_> {
    /// The program of `fix` for the files of `language`, or why the fix cannot
    /// be made in them.
    pub(super) fn program_of<'f>(
        &self,
        language: &str,
        fix: &'f CommandFix,
    ) -> Result<&'f Program, String> {
        if let Some(gap) = self.overlay_gap(language) {
            return Err(gap);
        }
        if let Some(missing) = fix
            .requires
            .iter()
            .find(|c| !self.engine.provides(language, **c))
        {
            return Err(format!(
                "language `{language}` lacks the capability `{missing}` the fix needs"
            ));
        }
        fix.programs
            .of(language)
            .ok_or_else(|| format!("the fix has no program for language `{language}`"))
    }

    /// The proposal of the command `fix` for `finding`, or why there is none.
    pub(super) fn propose_command(
        &self,
        outcome: &Outcome,
        finding: &Diagnostic,
        (binding, fix): (&FixBinding, &CommandFix),
        (sources, texts): (&Sources, &Overlays),
    ) -> Result<Proposal, Failure> {
        let fail = |reason: String| (Class::Unanswered, reason);
        let engine = self.engine;
        let language = outcome
            .project
            .file(&finding.file)
            .map_or("", |f| f.lang.as_str());
        let program = self.program_of(language, fix).map_err(fail)?;
        let text = sources.text(&finding.file).map_err(fail)?;
        let options = outcome
            .options
            .get(&finding.fingerprint)
            .cloned()
            .unwrap_or_default();
        let world = World::new(&engine.files, outcome, texts);
        let launch = Launch {
            program,
            finding,
            text: text.as_str(),
            options: &options,
            facts: outcome
                .project
                .file(&finding.file)
                .map_or(&NO_FACTS, |f| &f.facts),
        };
        let changes = self.rewrite(&launch, &world)?;
        let extents = if fix.scope == FixScope::Symbol {
            vec![
                choose_extent(&outcome.project, finding, text.as_str())
                    .map_err(|r| (Class::NoExtent, r))?,
            ]
        } else {
            Vec::new()
        };
        let bounds = Bounds {
            who: "the command",
            scope: fix.scope,
            file: &finding.file,
            extents: &extents,
        };
        let after = accept(changes, &bounds, &world)?;
        let (edits, bases) = edits_of(&after, &world)?;
        let made = Made {
            description: fix.description.clone(),
            safety: Safety::Safe,
            edits,
            bases,
            moves_only: false,
        };
        self.assembled(finding, binding, &made)
            .map_err(|reason| (Class::Unanswered, reason))
    }

    /// Runs the program for the finding and gathers what it changed.
    fn rewrite(&self, launch: &Launch<'_>, world: &World<'_>) -> Result<Changes, Failure> {
        let started = Instant::now();
        let changes = self.run_program(launch, world);
        self.command_time
            .set(self.command_time.get() + started.elapsed());
        changes
    }

    fn run_program(&self, launch: &Launch<'_>, world: &World<'_>) -> Result<Changes, Failure> {
        let program = launch.program;
        let workspace = &self.engine.ws;
        if is_filter(program) {
            return filter(launch, workspace);
        }
        let scratch = Scratch::new(&self.engine.files, world.texts).map_err(|e| {
            (
                Class::Unanswered,
                format!("the scratch copy of the project failed: {e}"),
            )
        })?;
        let ran = run(launch, workspace, scratch.path(), Some(scratch.path()))?;
        let changes = match program.output(Point::Fix) {
            Output::Stdout => filtered(launch, ran)?,
            _ => {
                let (changes, _) = collect(&scratch, world).map_err(|r| (Class::Unanswered, r))?;
                if changes.is_empty() {
                    return Err(unchanged(&ran));
                }
                changes
            }
        };
        Ok(changes)
    }
}
