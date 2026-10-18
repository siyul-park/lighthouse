//! The chain of runs of an agent, applied one at a time. Each run is judged
//! over the state the runs before it left, and undone by itself: when the
//! commands of `[fix] verify` fail, the newest run goes first, and then the
//! one before it, until they pass. What a run was applied over is checked
//! before it is, because it was made over the texts the earlier runs left.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use lighthouse_model::{Fingerprint, Preview};
use lighthouse_protocol::hash;

use super::{
    super::model::{Class, Event},
    super::propose::Proposal,
    super::{DeclinedFix, FixedBy, PreviewedFix},
    Orchestrator,
};
use crate::{Overlays, Run};

/// The state of a run at a point: its texts, the first text of every file it
/// changed, and its rounds.
pub(super) type Mark = (Overlays, BTreeMap<PathBuf, (String, String)>, usize);

/// The fixes of a pass by the link of the chain that made them.
pub(super) type Links = BTreeMap<Fingerprint, usize>;

impl Orchestrator<'_> {
    pub(super) fn mark(&self) -> Mark {
        (self.texts.clone(), self.original.clone(), self.rounds)
    }

    /// Whether a fix by a model stands.
    pub(super) fn has_model_fix(&self) -> bool {
        self.applied.iter().any(|a| a.fixer == FixedBy::Model)
    }

    /// The proposals of one link of the chain, less those made over texts that
    /// are no longer there, which are declined: an earlier link was not
    /// kept, or the files changed under it.
    pub(super) fn made_over_standing(&mut self, link: Vec<Proposal>) -> Vec<Proposal> {
        let mut fresh = Vec::new();
        for p in link {
            let Some(file) = self.stale_file(&p) else {
                fresh.push(p);
                continue;
            };
            let reason = format!(
                "`{file}` is not as the agent found it: an earlier edit of this pass was not kept"
            );
            self.proposer
                .note_model(&p.rule_id, Event::Declined(Class::Conflict));
            self.decline_proposal(&p, reason);
        }
        fresh
    }

    /// The first file `p` was made over that has another text now.
    fn stale_file(&self, p: &Proposal) -> Option<String> {
        let root = &self.proposer.engine.ws.root;
        p.bases.iter().find_map(|(file, base)| {
            let now = self
                .texts
                .get(file)
                .cloned()
                .or_else(|| fs::read_to_string(root.join(file)).ok());
            now.is_none_or(|text| hash::sha256(&text) != *base)
                .then(|| file.display().to_string())
        })
    }

    /// Undoes the newest link of the chain among the fixes by a model that
    /// stand, and declines its findings with `why`. False when none stands.
    pub(super) fn roll_back_newest(
        &mut self,
        outcome: &Run,
        links: &Links,
        marks: &BTreeMap<usize, Mark>,
        why: &str,
    ) -> bool {
        let newest = self
            .applied
            .iter()
            .filter(|a| a.fixer == FixedBy::Model)
            .filter_map(|a| links.get(&a.fingerprint))
            .max()
            .copied();
        let Some((newest, mark)) = newest.and_then(|n| Some((n, marks.get(&n)?))) else {
            return false;
        };
        (self.texts, self.original, _) = mark.clone();
        let (undone, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.applied)
            .into_iter()
            .partition(|a| a.fixer == FixedBy::Model && links.get(&a.fingerprint) == Some(&newest));
        self.applied = rest;
        let reason = format!("rolled back: {why}");
        for fix in &undone {
            self.previewable.remove(&fix.fingerprint);
            self.proposer
                .note_model(&fix.rule_id, Event::Declined(Class::RolledBack));
            if let Some(finding) = outcome
                .diagnostics
                .iter()
                .find(|d| d.fingerprint == fix.fingerprint)
            {
                self.declined.insert(
                    fix.fingerprint.clone(),
                    DeclinedFix {
                        fingerprint: fix.fingerprint.clone(),
                        rule_id: fix.rule_id.clone(),
                        file: finding.file.clone(),
                        line: finding.span.start.line,
                        reason: reason.clone(),
                    },
                );
            }
        }
        let files: Vec<String> = undone
            .iter()
            .flat_map(|fix| &fix.files)
            .map(|f| f.display().to_string())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self.notes
            .push(format!("rolled back {}: {why}", files.join(", ")));
        true
    }

    /// Puts the run back as it was at `mark`, before the fixes by a model, and
    /// moves those fixes from applied to previews. A fix of which no preview
    /// could be made is declined: a fix that was verified is applied,
    /// previewed or declined, never lost.
    pub(super) fn withdraw(&mut self, mark: Mark) {
        (self.texts, self.original, self.rounds) = mark;
        let (models, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.applied)
            .into_iter()
            .partition(|a| a.fixer == FixedBy::Model);
        self.applied = rest;
        for fix in models {
            match self.previewable.remove(&fix.fingerprint) {
                Some(Ok(preview)) => self.preview(fix.rule_id, preview),
                Some(Err(declined)) => self.decline_withdrawn(declined),
                None => {}
            }
        }
    }

    fn preview(&mut self, rule_id: String, preview: Preview) {
        self.proposer.note_model(&rule_id, Event::Previewed);
        self.previews.push(PreviewedFix { rule_id, preview });
    }

    fn decline_withdrawn(&mut self, declined: DeclinedFix) {
        self.proposer
            .note_model(&declined.rule_id, Event::Declined(Class::RolledBack));
        self.declined.insert(declined.fingerprint.clone(), declined);
    }
}
