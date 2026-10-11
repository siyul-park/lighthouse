//! The text of a round and its landing on disk: the formatted candidates, and
//! the verified result written in two phases.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use lighthouse_model::{Fingerprint, hash};
use lighthouse_resource::{write_atomic, write_atomic_guarded};
use lighthouse_spec::{Formatter, FormatterOutput, FormatterStdin};

use super::{
    super::edits::{TextEdit, apply},
    super::lower::Sources,
    super::{AppliedFix, DeclinedFix, FileChange, FixReport},
    Orchestrator, Proposal,
};
use crate::{Error, Outcome};

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
pub(super) struct Candidates {
    /// The new text of every file whose text changed.
    pub(super) texts: BTreeMap<PathBuf, String>,
    /// Their text before the round.
    pub(super) bases: BTreeMap<PathBuf, String>,
    /// Files the edits left as they were.
    pub(super) unchanged: BTreeSet<PathBuf>,
    /// Files whose edits could not be applied.
    pub(super) failed: BTreeMap<PathBuf, String>,
    /// What the user should know about how the round went.
    pub(super) notes: Vec<String>,
}

impl Orchestrator<'_> {
    /// Writes the changed files after the last round was verified, in two
    /// phases. First every file's hash is checked against what the run read, and
    /// a fix that spans a file that changed is skipped as a whole, before
    /// anything is written. Then each file is written atomically (containment
    /// and the hash are checked again right before the rename); if a write
    /// fails, the files already written are put back and nothing counts as
    /// applied. A dry run writes nothing.
    pub(super) fn finish(mut self, outcome: Result<Outcome, Error>) -> Result<FixReport, Error> {
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
            if let Err(e) = write_atomic_guarded(&target, &self.texts[path], &guard) {
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

    /// The new text of every file the kept proposals edit, formatted. Files
    /// whose edits cannot be applied, or whose formatter fails, are `failed`.
    pub(super) fn candidates(&self, kept: &[Proposal], before: &Outcome) -> Candidates {
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
