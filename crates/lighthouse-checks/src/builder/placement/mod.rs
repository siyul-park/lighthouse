//! The `placement` fact: where a free function belongs, decided once for the
//! whole project at four levels, coarsest first.
//!
//! - `module`: the project module whose symbols the function uses. The
//!   distance of a function `f` to a module `Q` is the share of the project
//!   symbols `f` uses that lie outside `Q`, a containment form of the entity
//!   distance of Tsantalis and Chatzigeorgiou (IEEE TSE 35(3), 2009), whose
//!   Jaccard form is dominated by the size of `Q`. A target must be
//!   admissible: moving `f` into it adds no import cycle (Martin's ADP), and
//!   every production caller can still reach it.
//! - `owner`: the type that is the function's receiver in all but name, the
//!   type of the module the function belongs in.
//! - `file`: the file of the module the function belongs in whose symbols it
//!   uses most; never a new file.
//! - `position` is not decided here: order decisions act after placement
//!   agrees.
//!
//! The coarsest wrong level is the `level` of the fact when a decision on by
//! default reports it (the owner), so the order decisions have nothing to say
//! about the symbol. A wrong module (`move`) and a wrong file (`file`) are
//! candidates for the decisions a model answers. A move is judged again as if
//! the function were already at its target; one that would itself be wrong
//! there is dropped.

mod judgement;
mod uses;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use lighthouse_model::ucm::{Model, Symbol, SymbolId, SymbolKind, Visibility};
use lighthouse_plugin::Workspace;
use serde_json::{Value, json};

use super::{Builder, Over};
use crate::{fact::Fact, indexes::Indexes, layout, library::Needs};
use judgement::{Candidate, Judgement, Owner, pick_file};
use uses::{Imports, Uses, share, signature, unit};

/// The fewest symbols of a module a function must use to belong in it.
const MIN_USES: usize = 2;
/// The least share of the symbols a function uses that its target must hold:
/// the distance to the current place minus the distance to the target.
const MIN_GAIN: f64 = 0.6;
/// The fewest statements a function must have to say where it belongs.
const MIN_STATEMENTS: u32 = 3;
/// The fewest members of a type a function must use to be its receiver.
const MIN_MEMBERS: usize = 3;

/// What the choice of a module reads: the function, where it lives and what it
/// uses, calls and takes.
struct Scope<'a> {
    symbol: &'a Symbol,
    home: &'a str,
    uses: &'a Uses<'a>,
    graph: &'a Imports,
    callers: &'a [&'a Symbol],
    takes: &'a BTreeSet<String>,
}

/// The `placement` of every free function of the project.
pub fn placements(
    model: &dyn Model,
    indexes: &dyn Indexes,
    ws: &Workspace,
) -> BTreeMap<SymbolId, Fact> {
    let over = Over { model, indexes, ws };
    let needs = Needs::default();
    let builder = Builder::over(over, &needs, "");
    builder
        .build_placements()
        .into_iter()
        .map(|(id, value)| (id, Fact::from(&value)))
        .collect()
}

impl Builder<'_> {
    /// The `placement` of a free function; `{}` for any other symbol.
    pub(super) fn placement(&self, symbol: &Symbol) -> Value {
        self.indexes
            .placement(&symbol.id)
            .map_or_else(judgement::absent, |placement| placement.json())
    }

    fn build_placements(&self) -> BTreeMap<SymbolId, Value> {
        let project = self.project;
        let graph = Imports::of(project);
        let mut judged: BTreeMap<&SymbolId, Judgement> = BTreeMap::new();
        for symbol in project.symbols().filter(|s| self.is_candidate(s)) {
            let unit = unit(project, symbol);
            let uses = Uses::of(project, symbol, &unit);
            let direct = Uses::of(project, symbol, &[symbol]);
            let mut judgement = self.judge(
                symbol,
                symbol.id.module(),
                &symbol.file,
                (&uses, &direct),
                &graph,
            );
            judgement.unit = unit.iter().skip(1).map(|s| s.id.clone()).collect();
            // The judgement is repeated as if the function were already at its
            // target: a target that would itself be a finding is not offered,
            // so the suggestions never bounce.
            if judgement.level == "module" {
                let (home, file) = judgement.destination(symbol);
                let again = self.judge(symbol, &home, &file, (&uses, &direct), &graph);
                // The owner is part of the target, so one found there is the
                // same suggestion once the function is a method.
                let bounces = match again.level {
                    "" => false,
                    "owner" => judgement.owner.is_none(),
                    _ => true,
                };
                if bounces {
                    judgement.level = "";
                }
            }
            judged.insert(&symbol.id, judgement);
        }
        // A helper that moves along with its only user is not a finding of its own.
        let moving: Vec<(String, Vec<SymbolId>)> = judged
            .iter()
            .filter(|(_, j)| j.level == "module")
            .map(|(_, j)| (j.target.clone(), j.unit.clone()))
            .collect();
        for (target, helpers) in moving {
            for helper in helpers {
                if let Some(j) = judged.get_mut(&helper)
                    && j.level == "module"
                    && j.target == target
                {
                    j.level = "";
                }
            }
        }
        let mut found = BTreeMap::new();
        for symbol in project.symbols().filter(|s| self.is_candidate(s)) {
            let Some(j) = judged.get(&symbol.id) else {
                continue;
            };
            found.insert(symbol.id.clone(), j.value(symbol));
        }
        found
    }

    /// A private free function of production code that is not generated and
    /// has enough statements to say where it belongs.
    fn is_candidate(&self, symbol: &Symbol) -> bool {
        let project = self.project;
        symbol.kind == SymbolKind::Function
            && symbol.owner.is_none()
            && !matches!(symbol.name.as_str(), "main" | "init")
            && !project.in_test(&symbol.id)
            && !project.in_example(&symbol.id)
            && symbol.visibility == Visibility::Private
            && project
                .function(&symbol.id)
                .is_some_and(|f| f.statements >= MIN_STATEMENTS)
            && !project.file(&symbol.file).is_some_and(|f| f.generated)
    }

    /// Judges `symbol` as if it lived in module `home` and file `file`.
    fn judge(
        &self,
        symbol: &Symbol,
        home: &str,
        file: &Path,
        (uses, direct): (&Uses, &Uses),
        graph: &Imports,
    ) -> Judgement {
        let project = self.project;
        let mut judgement = Judgement::new(home);
        let callers = self.production_callers(symbol);
        let (takes, external) = signature(project, symbol);
        let held = external
            || self.is_entry(symbol)
            || self.constructor_named(symbol)
            || !project.references(&symbol.id).is_empty()
            || callers.is_empty()
            || direct.total == 0;
        judgement.settled = held;
        let target_module = if held {
            home.to_owned()
        } else {
            let scope = Scope {
                symbol,
                home,
                uses,
                graph,
                callers: &callers,
                takes: &takes,
            };
            self.pick_module(&scope, &mut judgement)
        };
        if !held {
            judgement.owner = self.pick_owner(symbol, &target_module);
            // A helper lives with what it serves: one that every caller
            // sits beside stays in that file.
            let served = target_module == home
                && judgement.owner.is_none()
                && !callers.is_empty()
                && callers.iter().all(|c| c.file == file);
            let at = if target_module == home { direct } else { uses };
            judgement.file = if served {
                None
            } else {
                pick_file(project, at, (&target_module, home, file), &judgement)
            };
        }
        judgement.level = judgement.coarsest();
        judgement
    }

    /// Chooses the module the function belongs in, `home` when it stays.
    fn pick_module(&self, scope: &Scope, judgement: &mut Judgement) -> String {
        let Scope {
            symbol,
            home,
            uses,
            takes,
            ..
        } = *scope;
        let own = uses.own(home);
        let total = uses.total;
        let current = 1.0 - share(own, total);
        judgement.current_distance = current;
        let language = self.language_of(symbol);
        let used: Vec<&str> = uses
            .by_module
            .keys()
            .map(String::as_str)
            .filter(|m| *m != home)
            .collect();
        let mut candidates = Vec::new();
        for module in &used {
            let count = uses.count(module);
            // A move needs the target to be reachable as the receiver: the
            // function takes a value of one of its types.
            let reason = self
                .inadmissible(scope, module, language, &used)
                .or_else(|| {
                    (!takes.contains(*module)).then(|| format!("it takes no type of {module}"))
                })
                .or_else(|| {
                    takes
                        .iter()
                        .find(|other| *other != module)
                        .map(|other| format!("its signature also names a type of {other}"))
                });
            candidates.push(Candidate {
                module: (*module).to_owned(),
                behavior: uses.behavior(module),
                distance: 1.0 - share(count, total),
                reason,
            });
        }
        // The best admissible target: the smallest distance, then the
        // lexicographic order.
        candidates.sort_by(|a, b| {
            a.reason
                .is_some()
                .cmp(&b.reason.is_some())
                .then(a.distance.total_cmp(&b.distance))
                .then_with(|| a.module.cmp(&b.module))
        });
        let best = candidates
            .first()
            .filter(|c| c.reason.is_none() && c.behavior >= MIN_USES);
        let chosen = best
            .filter(|c| current - c.distance >= MIN_GAIN)
            .map(|c| c.module.clone());
        judgement.candidates = candidates;
        chosen.unwrap_or_else(|| home.to_owned())
    }

    /// Why a function cannot move into `module`: `None` when it can.
    fn inadmissible(
        &self,
        scope: &Scope,
        module: &str,
        language: &str,
        used: &[&str],
    ) -> Option<String> {
        let Scope {
            home,
            uses,
            graph,
            callers,
            ..
        } = *scope;
        let own = uses.own(home);
        let project = self.project;
        let info = project.module(module);
        if info.is_some_and(|m| m.name.as_deref() == Some("main") || m.path.contains("[bin:")) {
            return Some("a program is not imported".to_owned());
        }
        if info.is_some_and(|m| m.example || m.test_of.is_some()) {
            return Some("not production code".to_owned());
        }
        let declared = project
            .symbols()
            .find(|s| s.id.module() == module)
            .and_then(|s| project.language_of(s));
        if declared.is_some_and(|l| l != language) {
            return Some("another language".to_owned());
        }
        // ADP: the target must already import what the function uses, or not
        // be reachable from it.
        let others = used
            .iter()
            .copied()
            .filter(|m| *m != module)
            .chain((own > 0).then_some(home));
        for other in others {
            if other == module || layout::contains(other, module) {
                continue;
            }
            if !graph.direct(module, other) && graph.reaches(other, module) {
                return Some(format!("{module} would import {other}, which imports it"));
            }
        }
        // The callers must be able to import the target.
        for caller in callers {
            let from = caller.id.module();
            if from != module && !graph.direct(from, module) && graph.reaches(module, from) {
                return Some(format!("the caller in {from} would import a cycle"));
            }
        }
        None
    }

    /// The type the function is a method of, in the module it belongs in.
    fn pick_owner(&self, symbol: &Symbol, target: &str) -> Option<Owner> {
        let project = self.project;
        // Every production caller is a method of one type that the function
        // takes or uses: the receiver in all but name (`prefer-method`).
        let affinity = self.receiver_affinity(symbol);
        if let Some(owner) = affinity.get("owner").and_then(Value::as_str)
            && let Some(id) = SymbolId::parse(owner)
            && id.module() == target
            && (affinity["takes_owner_param"] == json!(true)
                || affinity["uses_owner"] == json!(true))
            && let Some(ty) = project.symbol(&id).filter(|t| t.kind == SymbolKind::Type)
        {
            return Some(Owner {
                id: ty.id.clone(),
                via: "callers",
            });
        }
        // Three or more members of one type and nothing of any other
        // (feature envy).
        let envy = self.envy(symbol);
        let id = envy.get("target").and_then(Value::as_str)?;
        let id = SymbolId::parse(id)?;
        let count = envy["count"].as_u64().unwrap_or(0) as usize;
        let alone = envy["others"].as_array().is_some_and(Vec::is_empty);
        let entry = envy["entry"] == json!(true);
        let ty = project.symbol(&id)?;
        (!entry && alone && count >= MIN_MEMBERS && id.module() == target).then(|| Owner {
            id: ty.id.clone(),
            via: "members",
        })
    }
}
