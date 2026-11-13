//! The oracle: one engine run over the scratch project of a cell and a
//! language, the findings read per package as the arrangements each decision
//! flags, and the verdict composed from them. The flags of a decision are
//! kept in the graph under everything they depend on, so an unchanged
//! decision is never run again.

use std::sync::Arc;

use lighthouse_graph::{Durability, Graph};
use lighthouse_logic::Assumption;
use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, Decision};
use serde_json::{Map, Value};

use super::{
    Alphabet, CellKey, FLAGS, Flags, FlagsKey, Language, Provenance, Scratch, Setting, VERDICT,
    Verdict, enumerate, run,
};

/// Runs the decisions of a cell over the shapes that can break their order.
pub struct Oracle<'a> {
    registry: Box<dyn Fn() -> Arc<Registry> + Send + Sync + 'a>,
    catalog: &'a Catalog,
}

/// The decisions that are in force together for one language, each with the
/// options it is configured with: one effective configuration.
pub struct Cell<'a> {
    pub language: &'a str,
    pub decisions: Vec<(&'a Decision, Map<String, Value>)>,
}

/// A verdict, and which decisions had to be run to give it: none when every
/// flags were kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Searched {
    pub verdict: Verdict,
    pub ran: Vec<String>,
}

/// A decision of the cell as the search keys it.
struct Member<'d> {
    assumption: Assumption,
    decision: &'d Decision,
    options: &'d Map<String, Value>,
}

impl<'a> Oracle<'a> {
    /// An oracle over `catalog` that builds a fresh registry from `registry`
    /// for every run.
    pub fn new(registry: impl Fn() -> Registry + Send + Sync + 'a, catalog: &'a Catalog) -> Self {
        Self {
            registry: Box::new(move || Arc::new(registry())),
            catalog,
        }
    }

    /// An oracle that runs every search over `registry`, the one a project's
    /// own engine runs with, shared between the runs.
    pub fn sharing(registry: Arc<Registry>, catalog: &'a Catalog) -> Self {
        Self {
            registry: Box::new(move || Arc::clone(&registry)),
            catalog,
        }
    }

    /// The contradictions among the shapes of up to `bound` declarations of
    /// `cell`. Whatever keeps the search from being done (no renderer, an
    /// alphabet that lacks a group, a plugin that crashed or could not read a
    /// file) is an incomplete verdict that says why, never a verdict.
    pub fn search(&self, graph: &mut Graph, cell: &Cell<'_>, bound: usize) -> Searched {
        self.searched(graph, cell, bound)
            .unwrap_or_else(|reason| Searched {
                verdict: Verdict::incomplete(vec![reason]),
                ran: Vec::new(),
            })
    }

    fn searched(
        &self,
        graph: &mut Graph,
        cell: &Cell<'_>,
        bound: usize,
    ) -> Result<Searched, String> {
        let language = Language::of(cell.language).map_err(|why| why.to_string())?;
        let pairs: Vec<(&Decision, &Map<String, Value>)> =
            cell.decisions.iter().map(|(d, o)| (*d, o)).collect();
        let alphabet = Alphabet::of(language, &pairs).map_err(|why| why.to_string())?;
        let registry = (self.registry)();
        let shapes = enumerate(language, alphabet, bound).map_err(|why| why.to_string())?;
        let provenance = Provenance {
            renderer: Scratch::digest(language, alphabet, &shapes),
            provider: run::provider(&registry),
            tool: crate::cache::build_identity(),
            injected: run::injected(self.catalog, language)?,
        };
        let setting = Setting::new(language, alphabet, bound, provenance);
        let members = members(language, &pairs)?;
        let missing = missing(graph, &setting, &members)?;
        let ran: Vec<String> = missing.iter().map(|m| m.assumption.uid.clone()).collect();
        if !missing.is_empty() {
            let rules: Vec<(&Decision, &Map<String, Value>)> =
                missing.iter().map(|m| (m.decision, m.options)).collect();
            let flags = run::flags(self.catalog, registry, (language, alphabet, bound), &rules)?;
            for member in &missing {
                let key = key(&setting, member);
                let found = flags.get(member.decision.id()).cloned().unwrap_or_default();
                graph
                    .set_input(&FLAGS, &key, Flags(found), Durability::Stable)
                    .map_err(|why| why.to_string())?;
            }
        }
        let cell = CellKey {
            setting,
            decisions: members.iter().map(|m| m.assumption.clone()).collect(),
        };
        let verdict = graph
            .query(&VERDICT, &cell)
            .map_err(|why| why.to_string())?;
        graph.commit().map_err(|why| why.to_string())?;
        Ok(Searched {
            verdict: (*verdict).clone(),
            ran,
        })
    }
}

/// The members of the cell, sorted by assumption.
fn members<'d>(
    language: Language,
    pairs: &[(&'d Decision, &'d Map<String, Value>)],
) -> Result<Vec<Member<'d>>, String> {
    let mut members = Vec::new();
    for &(decision, options) in pairs {
        let assumption = Assumption::of(decision, options, Some(language.id()))
            .map_err(|why| why.to_string())?;
        members.push(Member {
            assumption,
            decision,
            options,
        });
    }
    members.sort_by(|a, b| a.assumption.cmp(&b.assumption));
    Ok(members)
}

/// The members whose flags the graph does not hold.
fn missing<'m, 'd>(
    graph: &Graph,
    setting: &Setting,
    members: &'m [Member<'d>],
) -> Result<Vec<&'m Member<'d>>, String> {
    let mut missing = Vec::new();
    for member in members {
        let known = graph
            .input(&FLAGS, &key(setting, member))
            .map_err(|why| why.to_string())?;
        if known.is_none() {
            missing.push(member);
        }
    }
    Ok(missing)
}

fn key(setting: &Setting, member: &Member<'_>) -> FlagsKey {
    FlagsKey {
        assumption: member.assumption.clone(),
        setting: setting.clone(),
    }
}
