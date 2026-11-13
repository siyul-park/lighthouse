//! What the flags of the decisions of a cell say of the shapes: which shapes
//! no arrangement satisfies, the minimal sets of decisions that make them so,
//! and the smallest shape that shows each. The flags are inputs of the graph
//! (a decision's, once, under everything it depends on) and the verdict is a
//! query of them, so a changed decision is the only one run again.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_graph::{Error, Graph, Input, Query};
use lighthouse_logic::Assumption;
use serde::{Deserialize, Serialize};

use super::{
    Alphabet, Arrangement, Language, Shape, enumerate,
    search::{Search, Table},
};

/// What a decision flags in a scratch project, set once it ran.
pub static FLAGS: Input<FlagsKey, Flags> = Input::new("witness-flags").persisted();

/// The verdict on a cell, composed of the flags of its decisions.
pub static VERDICT: Query<CellKey, Verdict> = Query::new("witness-verdict", verdict);

/// What a search of a cell found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    /// The minimal sets of two or more decisions that cannot hold together,
    /// each with its smallest shape, in the order of their decisions.
    pub contradictions: Vec<Contradiction>,
    /// Decisions that flag every arrangement of a shape alone: a nogood of
    /// one, which also keeps every set that holds it from being reported.
    pub single: Vec<Single>,
    pub basis: Basis,
}

/// What the verdict rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Basis {
    /// No other contradiction exists among the shapes of up to
    /// `declarations`: never "proven consistent".
    Bounded(Bounded),
    /// The search could not be completed; why. The contradictions it found,
    /// if any, stand, and nothing else is said.
    Incomplete(Vec<String>),
}

/// The scope of a complete search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bounded {
    pub declarations: usize,
    pub alphabet: String,
    pub renderer: String,
    /// The decisions as the search saw them, each with the digest of what it
    /// asks and the options it ran with.
    pub options: Vec<Assumption>,
}

/// A set of decisions that cannot hold together and the smallest shape that
/// shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contradiction {
    /// The uids (or ids) of the decisions, sorted.
    pub decisions: Vec<String>,
    pub witness: Witness,
}

/// A shape every arrangement of which one of the decisions flags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Witness {
    pub shape: Shape,
    /// Each arrangement, the decisions of the contradiction that flag it and
    /// the text of its files.
    pub arrangements: Vec<Flagged>,
    /// The order a decision of the contradiction forces on two declarations:
    /// every arrangement that has them the other way round is flagged by it.
    pub forced: Vec<Forced>,
}

/// One arrangement of a witness and who flags it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flagged {
    pub arrangement: Arrangement,
    pub by: Vec<String>,
    pub text: String,
}

/// A decision that wants `first` declared above `second`, by index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forced {
    pub first: usize,
    pub second: usize,
    pub by: Vec<String>,
}

/// A decision that flags every arrangement of its smallest such shape: the
/// decision is unsatisfiable by itself there, whatever the order, and is
/// reported as its own nogood (`covering`, with: itself), never as a conflict
/// with another decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Single {
    pub decision: String,
    pub shape: Shape,
}

/// The decisions of a cell, sorted, under one [`Setting`]: the key of a
/// verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellKey {
    pub setting: Setting,
    pub decisions: Vec<Assumption>,
}

/// The key of the flags of one decision: its assumption (uid and the digest
/// of what it asks) under the setting.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FlagsKey {
    pub assumption: Assumption,
    pub setting: Setting,
}

/// What the search of a cell fixes besides its decisions: the key of flags and
/// of the verdict.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Setting {
    pub language: String,
    pub bound: usize,
    pub tests: bool,
    pub hook: Option<String>,
    /// The digest of the text of every package the shapes render to: moves
    /// when the renderer, the alphabet or the enumeration does.
    pub renderer: String,
    /// The plugins that read the rendered files and the CEL libraries the
    /// checks call, with their versions.
    pub provider: String,
    /// The build of Lighthouse: its fact builders and built-in checks.
    pub tool: String,
    /// The digest of the projects that inject the language, which the options
    /// of a decision do not hold (they decide the facts, not the decision), so
    /// they are part of the key and not of the assumption.
    pub injected: String,
}

/// What a search depends on besides its cell and bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub renderer: String,
    pub provider: String,
    pub tool: String,
    pub injected: String,
}

/// The packages of the scratch project a decision flags, by index, sorted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flags(pub Vec<usize>);

impl Setting {
    /// The setting of a search with this alphabet.
    pub fn new(
        language: Language,
        alphabet: Alphabet,
        bound: usize,
        provenance: Provenance,
    ) -> Self {
        Self {
            language: language.id().to_owned(),
            bound,
            tests: alphabet.tests,
            hook: alphabet.hook.map(str::to_owned),
            renderer: provenance.renderer,
            provider: provenance.provider,
            tool: provenance.tool,
            injected: provenance.injected,
        }
    }
}

impl Verdict {
    /// A verdict that says nothing: the search could not be done.
    pub fn incomplete(reasons: Vec<String>) -> Self {
        Self {
            contradictions: Vec::new(),
            single: Vec::new(),
            basis: Basis::Incomplete(reasons),
        }
    }
}

/// The contradictions among the shapes of the setting, given which packages
/// each decision flags.
pub(super) fn judge(setting: &Setting, flags: &BTreeMap<Assumption, BTreeSet<usize>>) -> Verdict {
    let Ok(language) = Language::of(&setting.language) else {
        return Verdict::incomplete(vec![format!("no renderer for `{}`", setting.language)]);
    };
    let alphabet = Alphabet {
        tests: setting.tests,
        hook: setting.hook.as_deref().and_then(super::hook_named),
    };
    let shapes = match enumerate(language, alphabet, setting.bound) {
        Ok(shapes) => shapes,
        Err(why) => return Verdict::incomplete(vec![why.to_string()]),
    };
    let table = Table::of(&shapes, flags);
    let search = Search::over(&table, flags.keys(), setting.bound);
    let basis = if search.reasons.is_empty() {
        Basis::Bounded(Bounded {
            declarations: setting.bound,
            alphabet: describe(language, alphabet),
            renderer: setting.renderer.clone(),
            options: flags.keys().cloned().collect(),
        })
    } else {
        Basis::Incomplete(search.reasons.clone())
    };
    Verdict {
        contradictions: search
            .nogoods
            .iter()
            .filter(|env| env.len() > 1)
            .map(|env| table.contradiction(env, search.shapes[env], language, alphabet))
            .collect(),
        single: search
            .nogoods
            .iter()
            .filter(|env| env.len() == 1)
            .flat_map(|env| env.iter().map(|decision| (decision, search.shapes[env])))
            .map(|(decision, at)| Single {
                decision: decision.uid.clone(),
                shape: table.rows[at].shape.clone(),
            })
            .collect(),
        basis,
    }
}

/// The verdict on the cell: the flags of each decision read, and judged.
fn verdict(graph: &Graph, cell: &CellKey) -> Result<Verdict, Error> {
    let mut flags = BTreeMap::new();
    for assumption in &cell.decisions {
        let key = FlagsKey {
            assumption: assumption.clone(),
            setting: cell.setting.clone(),
        };
        let Some(known) = graph.input(&FLAGS, &key)? else {
            let reason = format!("the flags of `{}` are not known", assumption.uid);
            return Ok(Verdict::incomplete(vec![reason]));
        };
        flags.insert(assumption.clone(), known.0.iter().copied().collect());
    }
    Ok(judge(&cell.setting, &flags))
}

fn describe(language: Language, alphabet: Alphabet) -> String {
    format!(
        "{}: declarations{}{}",
        language.id(),
        if alphabet.tests { ", tests" } else { "" },
        alphabet
            .hook
            .map_or(String::new(), |hook| format!(", hook {hook}")),
    )
}
