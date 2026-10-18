//! The fixes a run leaves for a hand to apply. A fix the run was not permitted
//! to write is kept in the result cache with the digests of the files it was
//! verified over, whoever made it: a rule, a command, a plugin or an agent.
//! Applying it later is writing exactly what was shown, if the files are still
//! as they were, and never making the fix again.

use lighthouse_model::{Diagnostic, Fingerprint, Safety};
use lighthouse_protocol::hash;

use super::{
    FixBinding, FixedBy,
    lower::Sources,
    model::Class,
    propose::{Proposal, Proposer},
};
use crate::cache::{Digest, KeyBuilder};

/// Why a fix to apply has no preview kept.
pub(super) const NOT_KEPT: &str = "the preview is not kept (run without --no-cache, or fix again)";

impl Proposer<'_> {
    /// Whether the run applies previews: it names findings and asks for
    /// `unsafe` fixes while the project does not write them unasked, where an
    /// `unsafe` fix is written only as it was shown. With `[fix] unsafe` the
    /// same words write a fix as it is made.
    pub(super) fn applies_previews(&self) -> bool {
        !self.run.fingerprints.is_empty()
            && self.run.unsafe_fixes == Some(true)
            && !self.engine.config.fix().unsafe_fixes
    }

    /// What applying previews makes of `finding`, if the run does: its kept
    /// preview, else why it has none. A fix that was never shown and may be
    /// unsafe is not made, and `None` leaves the finding to be fixed as ever.
    pub(super) fn applied_preview(
        &self,
        finding: &Diagnostic,
        binding: &FixBinding,
        sources: &Sources,
    ) -> Option<Result<Proposal, (Class, String)>> {
        if !self.applies_previews() {
            return None;
        }
        match self.kept(&finding.fingerprint, sources) {
            Ok(Some(proposal)) => Some(Ok(proposal)),
            Err(reason) => Some(Err((Class::Conflict, reason))),
            Ok(None) if binding.fixer == FixedBy::Model || binding.cap == Safety::Unsafe => {
                Some(Err((Class::Unanswered, NOT_KEPT.to_owned())))
            }
            Ok(None) => None,
        }
    }

    /// Keeps `proposal`, which the run withholds, for an application.
    pub(super) fn keep(&self, proposal: &Proposal) {
        if let Some(cache) = &self.kept {
            cache.put(key(&proposal.fingerprint), proposal);
        }
    }

    /// The preview kept for the finding `fingerprint`: `None` when none was
    /// made or the cache keeps nothing, and why not when a file it changes is
    /// not as it was verified over.
    pub(super) fn kept(
        &self,
        fingerprint: &Fingerprint,
        sources: &Sources,
    ) -> Result<Option<Proposal>, String> {
        let Some(proposal) = self
            .kept
            .as_ref()
            .and_then(|cache| cache.get::<Proposal>(&key(fingerprint)))
        else {
            return Ok(None);
        };
        let stale = proposal.bases.iter().find(|(file, base)| {
            !sources
                .text(file)
                .is_ok_and(|text| hash::sha256(text.as_str()) == **base)
        });
        match stale {
            Some((file, _)) => Err(format!(
                "`{}` is not as it was when the preview was made: apply the safe fixes first, or fix again for a new preview",
                file.display()
            )),
            None => Ok(Some(proposal)),
        }
    }

    /// Writes what the run kept; why it could not, if it could not.
    pub(super) fn finish_kept(&self) -> Option<String> {
        self.kept
            .as_ref()?
            .finish()
            .err()
            .map(|e| format!("the previews were not kept: {e}"))
    }
}

/// What a preview is kept under.
fn key(fingerprint: &Fingerprint) -> Digest {
    let mut key = KeyBuilder::new();
    key.part("fix-preview").part(fingerprint.as_str());
    key.finish()
}
