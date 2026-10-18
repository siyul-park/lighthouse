//! The descriptor of a decision, built in one place from the decision, the
//! project and what this machine can run: how its check and its fix run,
//! whoever makes them. Reports show it; this decides it.

use std::{cell::OnceCell, collections::BTreeMap};

use lighthouse_model::{Safety, Unavailable};
use lighthouse_process::sandbox_available;
use lighthouse_report::{Aim, Apply, CheckBy, CheckInfo, Descriptor, FixBy, FixInfo};
use lighthouse_spec::{Approval, Catalog, CheckKind, Decision, FixKind, Models};

use crate::{Result, Session, active_decisions, permit::permitted};

/// What the project and the machine decide about running checks and fixes,
/// read once for all the decisions a report describes.
pub struct Fixing {
    models: Models,
    trusted: bool,
    /// `[sandbox] enabled`: programs run in the sandbox, so it must work.
    sandboxed: bool,
    approval: Approval,
    /// The decisions `[fix] decisions` lists for a model fix.
    granted: Vec<String>,
    /// Whether the sandbox works here, found the first time a fix needs it.
    backend: OnceCell<bool>,
}

impl Fixing {
    /// The state of `session`'s project.
    pub fn of(session: &Session) -> Result<Self> {
        let active = active_decisions(session)?;
        let granted = permitted(&session.config, &session.catalog, &|id| active.contains(id))?;
        Ok(Self {
            models: session.config.models(session.catalog.projects())?,
            trusted: session.trusted(),
            sandboxed: session.config.sandbox().enabled,
            approval: session.config.fix().approval,
            granted: granted.ids.into_iter().collect(),
            backend: OnceCell::new(),
        })
    }

    /// The descriptors of the decisions `ids` name that have a check, by
    /// decision id, each aimed at all the findings of its decision.
    pub fn all<'a>(
        &self,
        catalog: &Catalog,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> BTreeMap<String, Descriptor> {
        ids.into_iter()
            .filter_map(|id| {
                let decision = catalog.decision(id)?;
                Some((id.to_owned(), self.describe(decision, Aim::Decision(id))?))
            })
            .collect()
    }

    /// How `decision` is checked and fixed here, with the fix aimed at `aim`;
    /// `None` for a decision without a check.
    pub fn describe(&self, decision: &Decision, aim: Aim<'_>) -> Option<Descriptor> {
        let check = decision.check.as_ref()?;
        let (by, runs) = match &check.kind {
            CheckKind::Builtin(_) => (CheckBy::Builtin, true),
            CheckKind::Cel(_) => (CheckBy::Cel, true),
            CheckKind::Command(_) => (CheckBy::Command, self.trusted),
            CheckKind::Model(_) => (
                CheckBy::Model,
                self.models.route(decision.id()).is_some()
                    || !self.models.classification.is_empty(),
            ),
        };
        let fix = self.fix(decision, aim);
        let brief = fix
            .as_ref()
            .is_none_or(|fix| !fix.runs || fix.safety == Safety::Unsafe);
        Some(Descriptor {
            check: CheckInfo { by, runs },
            fix,
            brief,
        })
    }

    /// The fix of `decision`: the one it declares, or a model's when the
    /// project lists the decision. `None` when it has neither.
    fn fix(&self, decision: &Decision, aim: Aim<'_>) -> Option<FixInfo> {
        let id = decision.id();
        let by = match decision.fix.as_ref().map(|fix| &fix.kind) {
            Some(FixKind::Ops { .. }) => FixBy::Ops,
            Some(FixKind::Command(_)) => FixBy::Command,
            Some(FixKind::Rpc { .. }) => FixBy::Rpc,
            Some(FixKind::Model { .. }) | None if self.granted.iter().any(|g| g == id) => {
                FixBy::Model
            }
            Some(FixKind::Model { .. }) | None => return None,
        };
        let safety = decision.fix_safety();
        let why = self.unavailable(by, id);
        Some(FixInfo {
            by,
            safety,
            runs: why.is_none(),
            why,
            approval: (safety == Safety::Unsafe).then_some(self.approval),
            apply: Apply::of(aim, safety == Safety::Unsafe),
        })
    }

    /// Why the fix by `by` of the decision `id` does not run here, if it does
    /// not: the same closed set for every provider.
    fn unavailable(&self, by: FixBy, id: &str) -> Option<Unavailable> {
        match by {
            FixBy::Rpc => Some(Unavailable::ProviderUnavailable),
            FixBy::Ops => None,
            FixBy::Model if self.models.edit_for(id).is_empty() => Some(Unavailable::NoEditBinding),
            FixBy::Model if self.models.budget == 0 => Some(Unavailable::BudgetSpent),
            FixBy::Command | FixBy::Model if !self.trusted => Some(Unavailable::UntrustedProject),
            FixBy::Command | FixBy::Model
                if self.sandboxed && !*self.backend.get_or_init(|| sandbox_available().is_ok()) =>
            {
                Some(Unavailable::SandboxUnavailable)
            }
            FixBy::Command | FixBy::Model => None,
        }
    }
}
