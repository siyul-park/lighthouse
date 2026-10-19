//! The fix by a command: the program of a decision's `fix` runs for one
//! finding, through the rewrite path it shares with an agent that edits (see
//! [`super::rewrite`]). A program that reads the file on stdin and prints its
//! new text (`output: stdout`) is a filter and needs no copy of the project;
//! every other runs in a scratch copy and leaves its change there. A change
//! within the scope of the fix is the proposal; a run that changed nothing
//! declines the finding, with what the program said as the reason.

use std::{collections::BTreeMap, path::Path};

use lighthouse_model::{Diagnostic, Safety};
use lighthouse_process::{Environment, Home, Job, Ran, context, expand};
use lighthouse_protocol::hash;
use lighthouse_spec::{FixScope, Output, Point, Stdin};
use serde_json::json;

use super::{
    CommandFix, FixBinding,
    edits::replace_changed,
    kept::NOT_KEPT,
    lower::Sources,
    model::{Class, choose_extent},
    propose::{Aim, Proposal, Proposer},
    rewrite::{Bounds, Change, Changes, Scratch, World, accept, collect},
};
use crate::{Overlays, Run};

/// How many characters of what a program said a reason carries.
const SAID_SHOWN: usize = 400;

/// Why a finding gets no fix by a command, and how that is counted.
type Failure = (Class, String);

/// What a run of the program is given.
struct Launch<'a> {
    fix: &'a CommandFix,
    finding: &'a Diagnostic,
    /// The text of the finding's file as it stands.
    text: &'a str,
    /// The options the finding's rule ran with.
    options: &'a serde_json::Map<String, serde_json::Value>,
}

impl Proposer<'_> {
    /// The proposal of the command `fix` for `finding`, or why there is none.
    pub(super) fn propose_command(
        &self,
        outcome: &Run,
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
        if let Some(gap) = self.overlay_gap(language) {
            return Err(fail(gap));
        }
        let provided = engine
            .language(language)
            .map(|provider| provider.manifest().capabilities.as_slice());
        if let Some(missing) = fix
            .requires
            .iter()
            .find(|c| !provided.is_some_and(|p| p.contains(c)))
        {
            return Err(fail(format!(
                "language `{language}` lacks the capability `{missing}` the fix needs"
            )));
        }
        let text = sources.text(&finding.file).map_err(fail)?;
        let options = outcome
            .options
            .get(&finding.fingerprint)
            .cloned()
            .unwrap_or_default();
        let world = World::new(&engine.ws.root, outcome, texts);
        let launch = Launch {
            fix,
            finding,
            text: text.as_str(),
            options: &options,
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
        let (edits, bases) = self.edits_of(&after, &world)?;
        let safety = self
            .plan
            .safety(&finding.rule_id, Safety::Safe, binding.cap);
        if safety == Safety::Unsafe && self.applies_previews() {
            return Err((Class::Unanswered, NOT_KEPT.to_owned()));
        }
        Ok(Proposal {
            fingerprint: finding.fingerprint.clone(),
            rule_id: finding.rule_id.clone(),
            file: finding.file.clone(),
            line: finding.span.start.line,
            fixer: binding.fixer.clone(),
            description: fix.description.clone(),
            safety,
            priority: self.plan.priority(&finding.rule_id),
            files: bases.keys().cloned().collect(),
            edits,
            aim: Aim::Finding,
            chain: 0,
            bases,
            batch: vec![finding.fingerprint.clone()],
        })
    }

    /// The text edits that turn each file of `after` from what stands into
    /// the program's version, and the digest of each as it stands.
    fn edits_of(
        &self,
        after: &BTreeMap<std::path::PathBuf, String>,
        world: &World<'_>,
    ) -> Result<
        (
            Vec<super::edits::TextEdit>,
            BTreeMap<std::path::PathBuf, String>,
        ),
        Failure,
    > {
        let mut edits = Vec::new();
        let mut bases = BTreeMap::new();
        for (path, new) in after {
            let old = world.text(path).ok_or_else(|| {
                (
                    Class::Conflict,
                    format!("`{}` cannot be read", path.display()),
                )
            })?;
            bases.insert(path.clone(), hash::sha256(&old));
            edits.extend(replace_changed(path, &old, new));
        }
        if edits.is_empty() {
            return Err((Class::Unchanged, "the command changed nothing".to_owned()));
        }
        Ok((edits, bases))
    }

    /// Runs the program for the finding and gathers what it changed.
    fn rewrite(&self, launch: &Launch<'_>, world: &World<'_>) -> Result<Changes, Failure> {
        let program = &launch.fix.program;
        let filter = program.stdin == Stdin::File && program.output(Point::Fix) == Output::Stdout;
        if filter {
            let ran = self.launch(launch, &self.engine.ws.root, None)?;
            return filtered(launch, ran);
        }
        let scratch = Scratch::new(&self.engine.ws.root, world.texts).map_err(|e| {
            (
                Class::Unanswered,
                format!("the scratch copy of the project failed: {e}"),
            )
        })?;
        let ran = self.launch(launch, scratch.path(), Some(scratch.path()))?;
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

    /// One run of the program in `dir`, where it may write only `writable`.
    fn launch(
        &self,
        launch: &Launch<'_>,
        dir: &Path,
        writable: Option<&Path>,
    ) -> Result<Ran, Failure> {
        let fail = |reason: String| (Class::Unanswered, reason);
        let engine = self.engine;
        let (fix, finding) = (launch.fix, launch.finding);
        let program = &fix.program;
        let shown = program.shown();
        let command = expand(&program.command, dir, |arg| match arg {
            "{file}" => Some(vec![finding.file.to_string_lossy().replace('\\', "/")]),
            "{line}" => Some(vec![finding.span.start.line.to_string()]),
            "{symbol}" => Some(vec![finding.symbol.clone().unwrap_or_default()]),
            "{decision}" => Some(vec![finding.rule_id.clone()]),
            _ => None,
        })
        .map_err(fail)?;
        let decision = json!({
            "decision": finding.rule_id,
            "message": finding.message,
            "file": finding.file,
            "line": finding.span.start.line,
            "symbol": finding.symbol,
            "fingerprint": finding.fingerprint.as_str(),
            "evidence": finding.evidence,
        });
        let environment = Environment::new(
            &program.env,
            context(
                &decision.to_string(),
                &serde_json::Value::Object(launch.options.clone()).to_string(),
            ),
            Home::User,
        )
        .map_err(fail)?;
        let policy = program.policy(&engine.ws.sandbox);
        let mut profile = policy.profile(&engine.ws.root, dir);
        profile.writes.extend(writable.map(Path::to_owned));
        let stdin = match program.stdin {
            Stdin::None => &[][..],
            Stdin::File => launch.text.as_bytes(),
        };
        let job = Job {
            command: &command,
            dir,
            stdin,
            environment: &environment,
            timeout: program.timeout(Point::Fix),
            policy: &policy,
            profile: &profile,
        };
        // A filter's stdout is the new text of a file, not something to read.
        let ran = if program.output(Point::Fix) == Output::Stdout {
            job.run_keeping_stdout()
        } else {
            job.run()
        }
        .map_err(|reason| fail(format!("`{shown}`: {reason}")))?;
        ran.exit(&program.success(Point::Fix))
            .map_err(|reason| fail(format!("`{shown}` {reason}")))?;
        Ok(ran)
    }
}

/// The change of a program whose stdout is the new text of the finding's
/// file.
fn filtered(launch: &Launch<'_>, ran: Ran) -> Result<Changes, Failure> {
    let new = std::str::from_utf8(&ran.stdout)
        .map_err(|_| (Class::Unanswered, "stdout is not valid UTF-8".to_owned()))?;
    if new == launch.text {
        return Err(unchanged(&ran));
    }
    if new.is_empty() {
        return Err((
            Class::Unchanged,
            "the command printed no text, so it changed nothing".to_owned(),
        ));
    }
    Ok(Changes::from([(
        launch.finding.file.clone(),
        Change::Text(new.to_owned()),
    )]))
}

/// A run that changed nothing, with what the program said as the reason.
fn unchanged(ran: &Ran) -> Failure {
    let said = String::from_utf8_lossy(&ran.stderr);
    let said: String = said.trim().chars().take(SAID_SHOWN).collect();
    let reason = if said.is_empty() {
        "the command changed no file".to_owned()
    } else {
        format!("the command changed no file: {said}")
    };
    (Class::Unchanged, reason)
}
