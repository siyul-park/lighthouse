//! The descriptor of a decision, built in one place from the decision, the
//! project and what this machine can run: how its check and its fix run,
//! whoever makes them. Reports show it; the fix plan of the engine and its
//! availability decide it, the same ones a fix run declines by.

use std::{collections::BTreeMap, path::Path};

use lighthouse_engine::{Availability, FixPlan, Needs};
use lighthouse_model::Safety;
use lighthouse_report::{Aim, Apply, CheckBy, CheckInfo, Descriptor, FixBy, FixInfo};
use lighthouse_spec::{Catalog, CheckKind, Decision, Models, Task};

use crate::{Result, Session, active_decisions, permit::permitted};

/// What the project and the machine decide about running checks and fixes,
/// read once for all the decisions a report describes.
pub struct Fixing {
    /// The fix of every decision that has one, models included for the
    /// decisions the project lists.
    plan: FixPlan,
    models: Models,
    trusted: bool,
    /// `[sandbox] enabled`: programs run in the sandbox, so it must work.
    sandboxed: bool,
    /// `[fix] unsafe`: an `unsafe` fix is written unasked.
    unsafe_fixes: bool,
}

impl Fixing {
    /// The state of `session`'s project.
    pub fn of(session: &Session) -> Result<Self> {
        let active = active_decisions(session)?;
        Ok(Self {
            plan: fix_plan(session, &|id| active.contains(id))?,
            models: session.config.models(session.catalog.projects())?,
            trusted: session.trusted(),
            sandboxed: session.config.sandbox().enabled,
            unsafe_fixes: session.config.fix().unsafe_fixes,
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
        let id = decision.id();
        let (by, needs) = match &decision.check.as_ref()?.kind {
            CheckKind::Builtin(_) => (CheckBy::Builtin, Needs::Nothing),
            CheckKind::Cel(_) => (CheckBy::Cel, Needs::Nothing),
            CheckKind::Command(_) if decision.bundled() => (CheckBy::Command, Needs::Bundled),
            CheckKind::Command(_) => (CheckBy::Command, Needs::Program),
            CheckKind::Model(_) => (
                CheckBy::Model,
                Needs::Model {
                    decision: id.to_owned(),
                    task: Task::Classification,
                },
            ),
        };
        let available = Availability::new(self.trusted, self.sandboxed, &self.models);
        let fix = self.plan.get(id).map(|binding| {
            let why = available.of(&binding.needs).err();
            FixInfo {
                by: fix_by(&binding.needs),
                safety: binding.cap,
                runs: why.is_none(),
                why,
                written: (binding.cap == Safety::Unsafe).then_some(self.unsafe_fixes),
                apply: Apply::of(aim, false),
            }
        });
        let brief = fix
            .as_ref()
            .is_none_or(|fix| !fix.runs || fix.safety == Safety::Unsafe);
        Some(Descriptor {
            check: CheckInfo {
                by,
                runs: available.of(&needs).is_ok(),
            },
            fix,
            brief,
        })
    }
}

/// Who makes a fix that needs `needs`.
pub(crate) fn fix_by(needs: &Needs) -> FixBy {
    match needs {
        Needs::Nothing => FixBy::Ops,
        Needs::Program | Needs::Bundled => FixBy::Command,
        Needs::Model { .. } => FixBy::Model,
        Needs::Unserved => FixBy::Rpc,
    }
}

/// The fix of every decision of the catalog: the one it declares, and a
/// model's for the decisions the project lists, of those `active` accepts.
pub(crate) fn fix_plan(session: &Session, active: &dyn Fn(&str) -> bool) -> Result<FixPlan> {
    let catalog = &session.catalog;
    let granted = permitted(&session.config, catalog, active)?;
    let plan = FixPlan::from_catalog(catalog);
    let plan = if granted.ids.is_empty() {
        plan
    } else {
        plan.with_model_fixes(catalog, &granted.ids, session.config.fix().scope)
    };
    let rules = session
        .config
        .resolve(Path::new(""), "", catalog.projects())?;
    Ok(plan.with_safety(
        rules
            .iter()
            .filter_map(|(id, rule)| Some((id.clone(), rule.safety?))),
    ))
}
