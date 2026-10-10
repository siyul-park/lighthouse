use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use lighthouse_model::hash::{self, Hasher};
use lighthouse_model::{Capability, Diagnostic, EditOp, Fingerprint, FixOutcome, Safety, Severity};
use lighthouse_plugin::{FixDecision, FixRequest};
use lighthouse_spec::write_atomic;
use serde_json::Value;

use super::{
    AppliedFix, DeclinedFix, FileChange, FixBinding, FixPlan, FixReport, FixRun, MAX_ROUNDS,
    edits::{TextEdit, apply, overlap, self_overlap},
    lower::{Lowerer, Sources},
};
use lighthouse_config::{Formatter, FormatterOutput, FormatterStdin};

use crate::{Engine, Error, Outcome, Overlays};

/// Settings files of formatters that look for them upward from the file.
const SETTINGS: [&str; 4] = [
    "rustfmt.toml",
    ".rustfmt.toml",
    ".editorconfig",
    "rust-toolchain.toml",
];

/// How long a formatter may run on one file.
const FORMAT_LIMIT: Duration = Duration::from_secs(60);

/// What a round did to the files its proposals touch.
#[derive(Default)]
struct Candidates {
    /// The new text of every file whose text changed.
    texts: BTreeMap<PathBuf, String>,
    /// Their text before the round.
    bases: BTreeMap<PathBuf, String>,
    /// Files the edits left as they were.
    unchanged: BTreeSet<PathBuf>,
    /// Files whose edits could not be applied.
    failed: BTreeMap<PathBuf, String>,
    /// What the user should know about how the round went.
    notes: Vec<String>,
}

/// A finding's fix, lowered and ready to apply.
pub(super) struct Proposal {
    fingerprint: Fingerprint,
    rule_id: String,
    /// Where the finding is, for reports.
    file: PathBuf,
    line: u32,
    fixer: String,
    pub(super) description: String,
    pub(super) safety: Safety,
    pub(super) edits: Vec<TextEdit>,
}

impl Proposal {
    fn files(&self) -> BTreeSet<PathBuf> {
        self.edits.iter().map(|e| e.file.clone()).collect()
    }

    fn sorted_edits(&self) -> Vec<&TextEdit> {
        let mut edits: Vec<&TextEdit> = self.edits.iter().collect();
        edits.sort_by(|a, b| {
            (&a.file, &a.range.start, &a.range.end).cmp(&(&b.file, &b.range.start, &b.range.end))
        });
        edits
    }
}

/// The state of one fix run.
pub(super) struct Orchestrator<'e> {
    engine: &'e Engine,
    plan: &'e FixPlan,
    run: &'e FixRun,
    /// The text of every file that stands in for the disk: nothing is written
    /// until the last round has been verified.
    texts: Overlays,
    /// The first text of every file the run changed, with its hash: what the
    /// disk must still hold when the run writes.
    original: BTreeMap<PathBuf, (String, String)>,
    /// Hashes of the states the rounds reached, to stop an oscillation.
    states: BTreeSet<String>,
    /// The project was trusted to run commands.
    trusted: bool,
    applied: Vec<AppliedFix>,
    declined: BTreeMap<Fingerprint, DeclinedFix>,
    notes: Vec<String>,
    rounds: usize,
}

impl Engine {
    /// Fixes the selected findings of the project: see the module docs. The
    /// analysis must be complete, because a fix that rests on a partial
    /// analysis is a guess.
    pub fn fix(&self, plan: &FixPlan, run: &FixRun) -> Result<FixReport, Error> {
        if let Some(id) = &run.fixer {
            if self.registry.fixer(id).is_none() {
                return Err(Error::Fix(format!("unknown fixer `{id}`")));
            }
            if run.rules.is_empty() && run.fingerprints.is_empty() {
                return Err(Error::Fix(
                    "a named fixer needs the rules or fingerprints it is for".to_owned(),
                ));
            }
        }
        let mut orchestrator = Orchestrator {
            engine: self,
            plan,
            run,
            texts: Overlays::new(),
            original: BTreeMap::new(),
            states: BTreeSet::new(),
            trusted: run.trusted,
            applied: Vec::new(),
            declined: BTreeMap::new(),
            notes: Vec::new(),
            rounds: 0,
        };
        let outcome = orchestrator.rounds();
        orchestrator.finish(outcome)
    }
}

impl<'e> Orchestrator<'e> {
    /// An orchestrator that only proposes: it holds no text and writes nothing.
    pub(super) fn for_preview(engine: &'e Engine, plan: &'e FixPlan, run: &'e FixRun) -> Self {
        Self {
            engine,
            plan,
            run,
            texts: Overlays::new(),
            original: BTreeMap::new(),
            states: BTreeSet::new(),
            trusted: false,
            applied: Vec::new(),
            declined: BTreeMap::new(),
            notes: Vec::new(),
            rounds: 0,
        }
    }
}

impl Orchestrator<'_> {
    /// Rounds until one applies nothing; the outcome of the last check.
    /// Candidates live in memory only, so an error leaves the disk as it was.
    fn rounds(&mut self) -> Result<Outcome, Error> {
        let mut checked: Option<Outcome> = None;
        self.states.insert(self.state_hash());
        for round in 1..=MAX_ROUNDS {
            let outcome = match checked.take() {
                Some(outcome) => outcome,
                None => self.engine.check_overlaid(&[], &[], &self.texts)?,
            };
            if round == 1 && !outcome.incomplete.is_empty() {
                let first = &outcome.incomplete[0];
                return Err(Error::Fix(format!(
                    "the analysis is incomplete ({}{}), so nothing was fixed",
                    first
                        .path
                        .as_ref()
                        .map_or(String::new(), |p| format!("{}: ", p.display())),
                    first.reason
                )));
            }
            let proposals = self.propose(&outcome)?;
            if proposals.is_empty() {
                return Ok(outcome);
            }
            let first_applied = self.applied.len();
            checked = self.apply_round(outcome, proposals, round)?;
            if checked.is_none() && self.rounds == round - 1 {
                // Everything this round was rolled back: stop rather than repeat it.
                return self.engine.check_overlaid(&[], &[], &self.texts);
            }
            if !self.states.insert(self.state_hash()) {
                let mut rules: Vec<&str> = self.applied[first_applied..]
                    .iter()
                    .map(|a| a.rule_id.as_str())
                    .collect();
                rules.sort_unstable();
                rules.dedup();
                self.notes.push(format!(
                    "stopped: the fixes of {} undo each other",
                    rules.join(", ")
                ));
                break;
            }
            if round == MAX_ROUNDS {
                self.notes.push(format!(
                    "stopped after {MAX_ROUNDS} rounds; run it again to continue"
                ));
            }
        }
        match checked {
            Some(outcome) => Ok(outcome),
            None => self.engine.check_overlaid(&[], &[], &self.texts),
        }
    }

    /// A hash of every text the run holds, which two rounds share only when
    /// the second undid the first.
    fn state_hash(&self) -> String {
        let mut hash = Hasher::new();
        for (path, text) in &self.texts {
            if self
                .original
                .get(path)
                .is_some_and(|(first, _)| first == text)
            {
                continue;
            }
            hash.update(path.to_string_lossy().as_bytes());
            hash.update([0]);
            hash.update(text.as_bytes());
            hash.update([0xff]);
        }
        hash.finish()
    }

    /// Writes the changed files after the last round was verified, in two
    /// phases. First every file's hash is checked against what the run read, and
    /// a fix that spans a file that changed is skipped as a whole, before
    /// anything is written. Then each file is written atomically (containment
    /// and the hash are checked again right before the rename); if a write
    /// fails, the files already written are put back and nothing counts as
    /// applied. A dry run writes nothing.
    fn finish(mut self, outcome: Result<Outcome, Error>) -> Result<FixReport, Error> {
        let outcome = outcome?;
        let changed: Vec<PathBuf> = self
            .texts
            .iter()
            .filter(|(path, text)| {
                self.original
                    .get(*path)
                    .is_some_and(|(first, _)| first != *text)
            })
            .map(|(path, _)| path.clone())
            .collect();
        let mut skipped: BTreeMap<PathBuf, String> = BTreeMap::new();
        if !self.run.dry_run {
            for path in &changed {
                let (_, hash) = &self.original[path];
                match fs::read_to_string(self.engine.ws.root.join(path)) {
                    Ok(now) if hash::sha256(&now) == *hash => {}
                    Ok(_) => {
                        skipped.insert(path.clone(), "changed concurrently".to_owned());
                    }
                    Err(e) => {
                        skipped.insert(path.clone(), format!("cannot be read: {e}"));
                    }
                }
            }
            self.skip_whole_fixes(&mut skipped);
            if let Some((failed, why)) = self.write_all(&changed, &skipped, &outcome) {
                for path in &changed {
                    skipped.insert(path.clone(), format!("was not written ({failed}: {why})"));
                }
            }
        }
        let written: Vec<&PathBuf> = changed
            .iter()
            .filter(|p| !skipped.contains_key(*p))
            .collect();
        let changes = written
            .iter()
            .map(|path| FileChange {
                path: (*path).clone(),
                before: self.original[*path].0.clone(),
                after: self.texts[*path].clone(),
            })
            .collect();
        for (path, why) in &skipped {
            self.notes
                .push(format!("{} was not written: {why}", path.display()));
        }
        let (kept, undone): (Vec<AppliedFix>, Vec<AppliedFix>) = std::mem::take(&mut self.applied)
            .into_iter()
            .partition(|a| !a.files.iter().any(|f| skipped.contains_key(f)));
        let still: BTreeSet<&Fingerprint> =
            outcome.diagnostics.iter().map(|d| &d.fingerprint).collect();
        self.declined
            .retain(|fingerprint, _| still.contains(fingerprint));
        for fix in undone {
            let why = fix
                .files
                .iter()
                .find_map(|f| skipped.get(f))
                .cloned()
                .unwrap_or_default();
            self.declined.insert(
                fix.fingerprint.clone(),
                DeclinedFix {
                    fingerprint: fix.fingerprint,
                    rule_id: fix.rule_id,
                    file: fix.files.first().cloned().unwrap_or_default(),
                    line: 0,
                    reason: format!("not written: the file {why}"),
                },
            );
        }
        Ok(FixReport {
            applied: kept,
            declined: self.declined.into_values().collect(),
            changes,
            rounds: self.rounds,
            notes: self.notes,
            finished: outcome,
        })
    }

    /// Adds to `skipped` every file of an applied fix that touches a skipped
    /// file: a fix that spans files is written whole or not at all.
    fn skip_whole_fixes(&self, skipped: &mut BTreeMap<PathBuf, String>) {
        loop {
            let mut grew = false;
            for fix in &self.applied {
                let Some(why) = fix.files.iter().find_map(|f| skipped.get(f)).cloned() else {
                    continue;
                };
                for file in &fix.files {
                    if !skipped.contains_key(file) {
                        skipped.insert(file.clone(), format!("{why}, and a fix spans it"));
                        grew = true;
                    }
                }
            }
            if !grew {
                return;
            }
        }
    }

    /// Writes every changed file that is not skipped. On a failure the files
    /// already written are restored and the failing file and reason returned.
    fn write_all(
        &self,
        changed: &[PathBuf],
        skipped: &BTreeMap<PathBuf, String>,
        outcome: &Outcome,
    ) -> Option<(String, String)> {
        let root = &self.engine.ws.root;
        let sources = Sources::new(root, &outcome.project, &self.texts);
        let mut done: Vec<&PathBuf> = Vec::new();
        for path in changed.iter().filter(|p| !skipped.contains_key(*p)) {
            let target = root.join(path);
            let (before, hash) = &self.original[path];
            let guard = || {
                sources.contain(path)?;
                match fs::read_to_string(&target) {
                    Ok(now) if hash::sha256(&now) == *hash => Ok(()),
                    _ => Err("changed concurrently".to_owned()),
                }
            };
            if let Err(e) =
                lighthouse_spec::write_atomic_guarded(&target, &self.texts[path], &guard)
            {
                for written in done {
                    let _ = write_atomic(&root.join(written), &self.original[written].0);
                }
                let _ = before;
                return Some((path.display().to_string(), e.to_string()));
            }
            done.push(path);
        }
        None
    }

    /// The proposals this round can try, with the findings that cannot be
    /// fixed recorded as declined.
    fn propose(&mut self, outcome: &Outcome) -> Result<Vec<Proposal>, Error> {
        let texts = self.texts.clone();
        let sources = Sources::new(&self.engine.ws.root, &outcome.project, &texts);
        let engine = self.engine;
        let complete = |language: &str| provides(engine, language, Capability::CompleteReferences);
        let lowerer = Lowerer::new(&outcome.project, &sources, &complete);
        let mut proposals = Vec::new();
        let candidates = self.select(outcome)?;
        for finding in candidates {
            let Some(binding) = self.binding(&finding.rule_id) else {
                self.decline(finding, "the rule has no fix".to_owned());
                continue;
            };
            // Eligibility comes first: a fix that would not be applied is never
            // even asked for, so a command never runs for nothing.
            if let Some(why) = &binding.unsupported {
                self.decline(finding, why.clone());
                continue;
            }
            if let Some(why) = self.ineligible(&binding) {
                self.decline(finding, why);
                continue;
            }
            match self.propose_one(outcome, finding, &binding, &lowerer, &sources) {
                Ok(proposal) if self.run.unsafe_fixes || proposal.safety == Safety::Safe => {
                    proposals.push(proposal);
                }
                Ok(proposal) => {
                    let reason = format!(
                        "the fixer proposes a {} fix ({}); it is applied only with unsafe fixes (`--unsafe-fixes`, MCP `unsafeFixes`)",
                        proposal.safety, proposal.description
                    );
                    self.decline(finding, reason);
                }
                Err(reason) => self.decline(finding, reason),
            }
        }
        Ok(proposals)
    }

    /// Why a fix may not be applied in this run, if it may not: judged from
    /// the binding alone.
    pub(super) fn ineligible(&self, binding: &FixBinding) -> Option<String> {
        if self.run.unsafe_fixes {
            return None;
        }
        let why = if binding.cap == Safety::Suggested {
            format!("its fix is {}", binding.cap)
        } else if !binding.mechanical {
            "its rule is not mechanical".to_owned()
        } else {
            return None;
        };
        Some(format!(
            "{why}, so it is applied only with unsafe fixes (`--unsafe-fixes`, MCP `unsafeFixes`)"
        ))
    }

    fn decline(&mut self, finding: &Diagnostic, reason: String) {
        self.declined.insert(
            finding.fingerprint.clone(),
            DeclinedFix {
                fingerprint: finding.fingerprint.clone(),
                rule_id: finding.rule_id.clone(),
                file: finding.file.clone(),
                line: finding.span.start.line,
                reason,
            },
        );
    }

    /// The findings the run was asked to fix. A finding of a rule without a
    /// fix is left out unless the selection names it.
    fn select<'o>(&self, outcome: &'o Outcome) -> Result<Vec<&'o Diagnostic>, Error> {
        let mut ignored = Vec::new();
        let scopes = self.engine.scopes(&self.run.paths, &mut ignored)?;
        let explicit = !self.run.rules.is_empty() || !self.run.fingerprints.is_empty();
        Ok(outcome
            .diagnostics
            .iter()
            .filter(|d| scopes.iter().any(|s| d.file.starts_with(s)))
            .filter(|d| self.run.rules.is_empty() || self.run.rules.contains(&d.rule_id))
            .filter(|d| {
                self.run.fingerprints.is_empty()
                    || self
                        .run
                        .fingerprints
                        .iter()
                        .any(|p| d.fingerprint.as_str().starts_with(p.as_str()))
            })
            .filter(|d| !self.run.skip.contains(&d.fingerprint))
            .filter(|d| explicit || self.binding(&d.rule_id).is_some())
            .collect())
    }

    fn binding(&self, rule: &str) -> Option<FixBinding> {
        match &self.run.fixer {
            Some(fixer) => Some(FixBinding {
                fixer: fixer.clone(),
                cap: Safety::Suggested,
                mechanical: false,
                unsupported: None,
                decision: FixDecision {
                    id: rule.to_owned(),
                    requirement: String::new(),
                    intent: String::new(),
                },
            }),
            None => self.plan.get(rule).cloned(),
        }
    }

    pub(super) fn propose_one(
        &self,
        outcome: &Outcome,
        finding: &Diagnostic,
        binding: &FixBinding,
        lowerer: &Lowerer,
        sources: &Sources,
    ) -> Result<Proposal, String> {
        let engine = self.engine;
        let fixer = engine
            .registry
            .fixer(&binding.fixer)
            .ok_or_else(|| format!("fixer `{}` is not registered", binding.fixer))?;
        let file = outcome.project.file(&finding.file);
        let language = file.map_or("", |f| f.lang.as_str());
        let provided = engine
            .registry
            .languages()
            .find(|(_, l)| l.manifest().id == language)
            .map(|(_, l)| l.manifest().capabilities.as_slice());
        if !provides(engine, language, Capability::Overlays) {
            return Err(format!(
                "the provider of `{language}` does not analyze overlays, so an edit cannot be verified before it is written"
            ));
        }
        if let Some(missing) = fixer
            .manifest()
            .requires
            .iter()
            .find(|c| !provided.is_some_and(|p| p.contains(c)))
        {
            return Err(format!(
                "language `{language}` lacks the capability `{missing}` the fix needs"
            ));
        }
        let text = sources.text(&finding.file)?;
        let facts = outcome
            .facts
            .get(&finding.fingerprint)
            .unwrap_or(&Value::Null);
        let options = outcome
            .options
            .get(&finding.fingerprint)
            .cloned()
            .unwrap_or_default();
        let request = FixRequest {
            finding,
            facts,
            decision: &binding.decision,
            options: &options,
            project: &outcome.project,
            ws: &engine.ws,
            text: &text,
            keys: &engine.registry,
            trusted: self.trusted,
        };
        match fixer
            .fix(&request)
            .map_err(|e| format!("the fixer failed: {e}"))?
        {
            FixOutcome::Declined { reason } => Err(reason),
            FixOutcome::Proposed {
                description,
                ops,
                safety,
            } => {
                let mut edits = Vec::new();
                for op in &ops {
                    edits.extend(
                        lowerer
                            .lower(op)
                            .map_err(|why| format!("{}: {why}", op_name(op)))?,
                    );
                }
                if edits.is_empty() {
                    return Err("the fix changes nothing".to_owned());
                }
                if self_overlap(&edits) {
                    return Err("the edits of the fix overlap each other".to_owned());
                }
                Ok(Proposal {
                    fingerprint: finding.fingerprint.clone(),
                    rule_id: finding.rule_id.clone(),
                    file: finding.file.clone(),
                    line: finding.span.start.line,
                    fixer: binding.fixer.clone(),
                    description,
                    safety: safety.capped(binding.cap),
                    edits,
                })
            }
        }
    }

    /// Applies what does not collide to the texts in memory, formats, verifies
    /// them with a check over overlays and drops what got worse. The check of
    /// the resulting state, when nothing was dropped.
    fn apply_round(
        &mut self,
        before: Outcome,
        mut proposals: Vec<Proposal>,
        round: usize,
    ) -> Result<Option<Outcome>, Error> {
        // Safe fixes win a collision, so the ones that apply unasked are not
        // shut out by a suggestion.
        proposals.sort_by_key(|p| p.safety);
        let (kept, merged) = kept_proposals(proposals);
        let mut candidates = self.candidates(&kept, &before);
        let mut failed = std::mem::take(&mut candidates.failed);
        self.notes.append(&mut candidates.notes);
        let mut trial = self.texts.clone();
        trial.extend(candidates.texts.clone());
        let after = if failed.len() == kept.len() && candidates.texts.is_empty() {
            None
        } else {
            Some(self.engine.check_overlaid(&[], &[], &trial)?)
        };
        let changed: Vec<PathBuf> = candidates.texts.keys().cloned().collect();
        if let Some(after) = &after {
            failed.extend(worse(&before, after, &changed));
        }
        let rolled = close_over(&kept, failed);
        for (file, reason) in &rolled {
            candidates.texts.remove(file);
            self.notes
                .push(format!("rolled back {}: {reason}", file.display()));
        }
        for (file, text) in &candidates.texts {
            self.original.entry(file.clone()).or_insert_with(|| {
                let base = candidates.bases[file].clone();
                let hash = hash::sha256(&base);
                (base, hash)
            });
            self.texts.insert(file.clone(), text.clone());
        }
        if self.account(&kept, &merged, &rolled, &candidates.unchanged, round) {
            self.rounds = round;
        }
        Ok(if rolled.is_empty() { after } else { None })
    }

    /// The new text of every file the kept proposals edit, formatted. Files
    /// whose edits cannot be applied, or whose formatter fails, are `failed`.
    fn candidates(&self, kept: &[Proposal], before: &Outcome) -> Candidates {
        let sources = Sources::new(&self.engine.ws.root, &before.project, &self.texts);
        let mut by_file: BTreeMap<PathBuf, Vec<&TextEdit>> = BTreeMap::new();
        for edit in kept.iter().flat_map(|p| &p.edits) {
            by_file.entry(edit.file.clone()).or_default().push(edit);
        }
        let mut out = Candidates::default();
        let mut unformatted = false;
        for (file, edits) in &by_file {
            let base = match sources.text(file) {
                Ok(base) => base,
                Err(why) => {
                    out.failed.insert(file.clone(), why);
                    continue;
                }
            };
            let analyzed = before.project.file(file).map(|f| f.hash.as_str());
            if analyzed.is_some_and(|hash| hash != hash::sha256(base.as_bytes())) {
                out.failed
                    .insert(file.clone(), "changed concurrently".to_owned());
                continue;
            }
            let next = match apply(&base, edits) {
                Ok(next) if next == *base => {
                    out.unchanged.insert(file.clone());
                    continue;
                }
                Ok(next) => next,
                Err(why) => {
                    out.failed.insert(file.clone(), why);
                    continue;
                }
            };
            let language = before.project.file(file).map_or("", |f| f.lang.as_str());
            let next = match self.format(file, language, next, &mut unformatted, &mut out.notes) {
                Ok(next) => next,
                Err(why) => {
                    out.failed.insert(file.clone(), why);
                    continue;
                }
            };
            out.bases.insert(file.clone(), (*base).clone());
            out.texts.insert(file.clone(), next);
        }
        out
    }

    /// Records what became of each kept proposal and those merged into it:
    /// applied, or declined with the reason. True when something stayed applied.
    fn account(
        &mut self,
        kept: &[Proposal],
        merged: &[(Proposal, Fingerprint)],
        rolled: &BTreeMap<PathBuf, String>,
        unchanged: &BTreeSet<PathBuf>,
        round: usize,
    ) -> bool {
        let mut any = false;
        for proposal in kept {
            let files = proposal.files();
            let reason = match files.iter().find(|f| rolled.contains_key(*f)) {
                Some(file) => Some(format!("rolled back: {}", rolled[file])),
                None if files.iter().all(|f| unchanged.contains(f)) => {
                    Some("the fix changes nothing".to_owned())
                }
                None => None,
            };
            let covered = merged
                .iter()
                .filter(|(_, into)| *into == proposal.fingerprint)
                .map(|(p, _)| p);
            for p in std::iter::once(proposal).chain(covered) {
                if let Some(reason) = &reason {
                    self.decline_proposal(p, reason.clone());
                    continue;
                }
                any = true;
                self.applied.push(AppliedFix {
                    fingerprint: p.fingerprint.clone(),
                    rule_id: p.rule_id.clone(),
                    fixer: p.fixer.clone(),
                    description: p.description.clone(),
                    safety: p.safety,
                    files: files.iter().cloned().collect(),
                    round,
                });
            }
        }
        any
    }

    fn decline_proposal(&mut self, p: &Proposal, reason: String) {
        self.declined.insert(
            p.fingerprint.clone(),
            DeclinedFix {
                fingerprint: p.fingerprint.clone(),
                rule_id: p.rule_id.clone(),
                file: p.file.clone(),
                line: p.line,
                reason,
            },
        );
    }

    /// Runs the language's formatter and takes the result. It runs with the
    /// project root as its working directory, so version managers resolve and a
    /// stdin formatter finds the project's settings from there; `{path}` is the
    /// file's real project-relative path, for tools that take one. A formatter
    /// that works on a file gets a scratch copy of it, `{file}`, next to copies
    /// of the settings files found between it and the root. Formatting needs a
    /// trusted project; without trust it is skipped with a note, and
    /// verification still runs on the unformatted text.
    fn format(
        &self,
        file: &Path,
        language: &str,
        text: String,
        skipped: &mut bool,
        notes: &mut Vec<String>,
    ) -> Result<String, String> {
        let Some(formatter) = self.engine.config.formatter(language) else {
            return Ok(text);
        };
        let shown = formatter.argv.join(" ");
        if !self.trusted {
            if !*skipped {
                *skipped = true;
                notes.push(format!(
                    "not formatted: the formatter `{shown}` runs only in a trusted project (`lighthouse trust`)"
                ));
            }
            return Ok(text);
        }
        self.run_formatter(formatter, file, &text)
    }

    /// Runs `formatter` on `text` of `file`, as the command contract says.
    fn run_formatter(
        &self,
        formatter: &Formatter,
        file: &Path,
        text: &str,
    ) -> Result<String, String> {
        let scratch = tempfile::tempdir().map_err(|e| e.to_string())?;
        let copy = scratch.path().join(file);
        if let Some(dir) = copy.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        fs::write(&copy, text).map_err(|e| e.to_string())?;
        self.copy_settings(file, scratch.path());
        let path = copy.to_string_lossy().into_owned();
        let real = file.to_string_lossy().replace('\\', "/");
        let mut command: Vec<String> = formatter
            .argv
            .iter()
            .map(|a| match a.as_str() {
                "{file}" => path.clone(),
                "{path}" => real.clone(),
                other => other.to_owned(),
            })
            .collect();
        let named = formatter.argv.iter().any(|a| a == "{file}");
        if formatter.stdin == FormatterStdin::None && !named {
            command.push(path);
        }
        let stdin = match formatter.stdin {
            FormatterStdin::None => Vec::new(),
            FormatterStdin::File => text.as_bytes().to_vec(),
        };
        let mut env: Vec<(String, String)> = ["PATH", "HOME", "LANG", "TMPDIR"]
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_owned(), v)))
            .collect();
        env.extend(formatter.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        let output = lighthouse_process::run(&lighthouse_process::Spec {
            argv: &command,
            dir: &self.engine.ws.root,
            stdin: &stdin,
            env: Some(&env),
            limit: FORMAT_LIMIT,
            grace: Duration::from_secs(2),
            max_output: lighthouse_process::MAX_OUTPUT,
        })
        .map_err(|e| format!("formatter `{}`: {e}", formatter.argv[0]))?;
        if !output.success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "formatter `{}` failed: {}",
                formatter.argv[0],
                stderr.lines().next().unwrap_or("no message")
            ));
        }
        match formatter.output {
            FormatterOutput::InPlace => {
                fs::read_to_string(&copy).map_err(|e| format!("formatter output: {e}"))
            }
            FormatterOutput::Text => String::from_utf8(output.stdout)
                .map_err(|_| "the formatter printed text that is not UTF-8".to_owned())
                .and_then(|out| {
                    if out.is_empty() {
                        Err("the formatter printed no text".to_owned())
                    } else {
                        Ok(out)
                    }
                }),
        }
    }
    /// Copies the formatter settings files that apply to `file`, found by
    /// walking up from its directory to the project root, to the same places in
    /// `scratch`, so a tool that looks for them upward from the file finds them.
    fn copy_settings(&self, file: &Path, scratch: &Path) {
        let root = &self.engine.ws.root;
        let mut dir = file.parent();
        while let Some(rel) = dir {
            for name in SETTINGS {
                let from = root.join(rel).join(name);
                if from.is_file() {
                    let to = scratch.join(rel).join(name);
                    if let Some(parent) = to.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::copy(&from, &to);
                }
            }
            dir = rel.parent().filter(|p| p != &rel);
            if rel.as_os_str().is_empty() {
                break;
            }
        }
    }
}

/// Whether the provider of `language` declares `capability`.
pub(super) fn provides(engine: &Engine, language: &str, capability: Capability) -> bool {
    engine
        .registry
        .languages()
        .find(|(_, l)| l.manifest().id == language)
        .is_some_and(|(_, l)| l.manifest().capabilities.contains(&capability))
}

fn op_name(op: &EditOp) -> &'static str {
    match op {
        EditOp::Move { .. } => "move",
        EditOp::Delete { .. } => "delete",
        EditOp::Reorder { .. } => "reorder",
        EditOp::Rename { .. } => "rename",
        EditOp::DeleteRange { .. } => "delete_range",
        EditOp::Replace { .. } => "replace",
    }
}

/// Proposals in order, dropping the ones that collide with an earlier one. A
/// proposal with exactly the edits of a kept one is covered by it, and
/// returned with the fingerprint it merged into.
fn kept_proposals(proposals: Vec<Proposal>) -> (Vec<Proposal>, Vec<(Proposal, Fingerprint)>) {
    let mut kept: Vec<Proposal> = Vec::new();
    let mut merged = Vec::new();
    for proposal in proposals {
        let same = kept.iter().find(|k| {
            let (a, b) = (k.sorted_edits(), proposal.sorted_edits());
            a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x == y)
        });
        if let Some(same) = same {
            merged.push((proposal, same.fingerprint.clone()));
            continue;
        }
        let collides = proposal.edits.iter().any(|e| {
            kept.iter()
                .flat_map(|k| &k.edits)
                .any(|k| k.file == e.file && overlap(&k.range, &e.range))
        });
        if !collides {
            kept.push(proposal);
        }
    }
    (kept, merged)
}

/// The changed files whose check got worse: more findings of error or warn
/// severity for some rule and owner (the symbol, or the file for a finding with
/// none; a fingerprint can change when code moves, its owner does not), or a new
/// gap in the analysis. A change that cannot be pinned on one file (a gap with
/// no path, a finding in an untouched file) condemns them all.
fn worse(before: &Outcome, after: &Outcome, changed: &[PathBuf]) -> BTreeMap<PathBuf, String> {
    type Key = (PathBuf, String, String);
    let count = |o: &Outcome| -> BTreeMap<Key, usize> {
        let mut counts = BTreeMap::new();
        let loud = o
            .diagnostics
            .iter()
            .filter(|d| matches!(d.severity, Severity::Error | Severity::Warn));
        for d in loud {
            let owner = d.symbol.clone().unwrap_or_default();
            *counts
                .entry((d.file.clone(), d.rule_id.clone(), owner))
                .or_insert(0) += 1;
        }
        counts
    };
    let known = count(before);
    let gaps: BTreeSet<_> = before.incomplete.iter().collect();
    let mut bad: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut everyone: Option<String> = None;
    for (key, now) in &count(after) {
        if known.get(key).copied().unwrap_or(0) >= *now {
            continue;
        }
        let (file, rule, _) = key;
        let reason = format!("it introduced a finding of {rule}");
        if changed.contains(file) {
            bad.entry(file.clone()).or_insert(reason);
        } else {
            everyone.get_or_insert_with(|| format!("{reason} in {}", file.display()));
        }
    }
    for gap in after.incomplete.iter().filter(|g| !gaps.contains(g)) {
        match &gap.path {
            Some(path) if changed.contains(path) => {
                bad.entry(path.clone()).or_insert_with(|| {
                    format!("the file is no longer fully analyzed: {}", gap.reason)
                });
            }
            _ => {
                everyone.get_or_insert_with(|| {
                    format!("the analysis became incomplete: {}", gap.reason)
                });
            }
        }
    }
    if let Some(reason) = everyone {
        for file in changed {
            bad.entry(file.clone()).or_insert_with(|| reason.clone());
        }
    }
    bad
}

/// `failed` plus every file that shares a proposal with a failed one: a fix
/// that spans files is undone as a whole.
fn close_over(
    kept: &[Proposal],
    mut failed: BTreeMap<PathBuf, String>,
) -> BTreeMap<PathBuf, String> {
    loop {
        let mut grew = false;
        for proposal in kept {
            let files = proposal.files();
            let Some(reason) = files.iter().find_map(|f| failed.get(f)).cloned() else {
                continue;
            };
            for file in files {
                if let std::collections::btree_map::Entry::Vacant(slot) = failed.entry(file) {
                    slot.insert(reason.clone());
                    grew = true;
                }
            }
        }
        if !grew {
            return failed;
        }
    }
}
