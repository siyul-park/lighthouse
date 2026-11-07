//! Fixes as queries: `fix(rule, finding)` is the edits of one finding, lowered
//! to text and held to the scope of its fix. What the fixer reads of the code
//! model, of the indexes and of the text of the file is recorded as the
//! dependencies of the node, so a round whose edits moved none of them
//! proposes nothing again, and an undo finds its nodes as they were. A fix by
//! a command that reads the file on stdin and prints its new text is the same
//! node, keyed on the program as well: it is run again when the text of the
//! file or the program moves, and a query that starts a program says so, as
//! the check of a command does.
//!
//! What the node is asked is its key: the finding with the facts and the
//! options it was judged with, and for a command the program. How the fix of
//! the rule is made is the input `fixing`, set by the host each round: it
//! moves exactly the nodes of the rule.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_graph::{Error, Graph, Input, Query, VERSION};
use lighthouse_model::{
    Diagnostic, Options, Safety,
    ucm::{Capability, EditOp, FixOutcome, Held, Model, View},
};
use lighthouse_plugin::{FixDecision, FixRequest, Runtime};
use lighthouse_protocol::hash;
use lighthouse_spec::{FixScope, Program};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    edits::{TextEdit, apply, keeps_declarations, self_overlap},
    lower::{Lowerer, Sources},
    model::choose_extent,
    program::{self, Launch, NO_FACTS},
    rewrite::{Bounds, Change, Changes, Standing, accept},
};

/// How the fix of a rule is made. Set by the host for each rule with a fix of
/// its own. `Stable`.
pub(super) static FIXING: Input<String, Fixing> = Input::new("fixing");

/// The fix of a finding, by the rule and the finding as [`Ask`] states it in
/// JSON.
pub(super) static FIX: Query<(String, String), Fixed> =
    Query::new("fix", fix).starting_program(runs_program);

/// How the fix of one rule is made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Fixing {
    /// The registered fixer that proposes; empty for a command, which the
    /// asks carry the program of.
    pub(super) fixer: String,
    pub(super) scope: FixScope,
    /// What a command fix does, in the words of the decision's title.
    pub(super) description: String,
    pub(super) requirement: String,
    pub(super) context: String,
    /// What the fix depends on besides the code: the plugins, the languages
    /// as configured and the decision as the catalog has it.
    pub(super) revision: String,
}

/// What a fix is asked for: a finding as the check judged it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Ask {
    pub(super) finding: Diagnostic,
    /// What the analysis knew about the finding's subject.
    pub(super) facts: Value,
    /// The options the finding's rule ran with.
    pub(super) options: Options,
    /// The program of a fix by a command, for the language of the file.
    pub(super) program: Option<Program>,
}

impl Ask {
    /// The key of the node that answers this for the rule `rule`.
    pub(super) fn key(&self) -> (String, String) {
        (
            self.finding.rule_id.clone(),
            serde_json::to_string(self).unwrap_or_default(),
        )
    }
}

/// What a fix came to: the edits of the finding, else why it has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Fixed {
    Declined(String),
    Made(Made),
}

/// The edits of a fix, as lowered, and what it says of itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Made {
    pub(super) description: String,
    /// The safety the fixer claims, before the project's word.
    pub(super) safety: Safety,
    pub(super) edits: Vec<TextEdit>,
    /// The digest of each file the edits are in, as the fix was made over it.
    pub(super) bases: BTreeMap<PathBuf, String>,
    /// The edits come from operations that only move declarations.
    pub(super) moves_only: bool,
}

/// The project as the node reads it, for the scope of a fix.
struct Stands<'a> {
    view: &'a dyn Model,
    sources: &'a Sources<'a>,
}

impl Standing for Stands<'_> {
    fn project(&self) -> &dyn Model {
        self.view
    }

    fn text(&self, path: &std::path::Path) -> Option<String> {
        self.sources.text(path).ok().map(|text| text.to_string())
    }
}

/// Whether the node runs a program.
fn runs_program(_: &Graph, (_, ask): &(String, String)) -> bool {
    serde_json::from_str::<Ask>(ask).is_ok_and(|ask| ask.program.is_some())
}

fn fix(graph: &Graph, (rule, ask): &(String, String)) -> Result<Fixed, Error> {
    graph.input(&VERSION, &"binary".to_owned())?;
    let fixing = graph
        .input(&FIXING, rule)?
        .ok_or_else(|| Error::Encode(format!("{rule}: the fix is not declared")))?;
    let ask: Ask = serde_json::from_str(ask)
        .map_err(|e| Error::Encode(format!("{rule}: the finding cannot be read: {e}")))?;
    let runtime = Runtime::of(graph)?;
    let held = Held::default();
    let view = View::new(graph, &held)?;
    let sources = Sources::graph(&runtime.workspace().root, &view, graph);
    let made = match &ask.program {
        Some(_) => commanded(&ask, &fixing, (runtime, &view as &dyn Model, &sources)),
        None => proposed(
            graph,
            &ask,
            &fixing,
            (runtime, &view as &dyn Model, &sources),
        ),
    };
    if let Some(error) = view.failure() {
        return Err(error);
    }
    Ok(match made {
        Ok(made) => Fixed::Made(made),
        Err(reason) => Fixed::Declined(reason),
    })
}

/// The fix a registered fixer proposes, lowered.
fn proposed(
    graph: &Graph,
    ask: &Ask,
    fixing: &Fixing,
    (runtime, view, sources): (&Runtime, &dyn Model, &Sources),
) -> Result<Made, String> {
    let finding = &ask.finding;
    let fixer = runtime
        .registry()
        .fixer(&fixing.fixer)
        .ok_or_else(|| format!("fixer `{}` is not registered", fixing.fixer))?;
    let text = sources.text(&finding.file)?;
    let decision = FixDecision {
        id: finding.rule_id.clone(),
        requirement: fixing.requirement.clone(),
        context: fixing.context.clone(),
    };
    let queries = lighthouse_plugin::queries(graph).map_err(|e| format!("{e}"))?;
    let request = FixRequest {
        finding,
        facts: &ask.facts,
        decision: &decision,
        options: &ask.options,
        project: view,
        queries: &queries,
        workspace: runtime.workspace(),
        text: &text,
        trusted: runtime.trusted(),
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
            let complete =
                |language: &str| provides(runtime, language, Capability::CompleteReferences);
            let lowerer = Lowerer::new(view, sources, &complete);
            let (edits, bases) = lowered(&ops, &lowerer, sources)?;
            within(fixing.scope, finding, &edits, (view, sources))?;
            Ok(Made {
                description,
                safety,
                edits,
                bases,
                moves_only: moves_only(&ops),
            })
        }
    }
}

/// The fix of a program that prints the new text of the file.
fn commanded(
    ask: &Ask,
    fixing: &Fixing,
    (runtime, view, sources): (&Runtime, &dyn Model, &Sources),
) -> Result<Made, String> {
    let (finding, Some(program)) = (&ask.finding, &ask.program) else {
        return Err("the fix has no program".to_owned());
    };
    let text = sources.text(&finding.file)?;
    let launch = Launch {
        program,
        finding,
        text: text.as_str(),
        options: &ask.options,
        facts: view.file(&finding.file).map_or(&NO_FACTS, |f| &f.facts),
    };
    let changes = program::filter(&launch, runtime.workspace()).map_err(|(_, why)| why)?;
    let extents = if fixing.scope == FixScope::Symbol {
        vec![choose_extent(view, finding, text.as_str())?]
    } else {
        Vec::new()
    };
    let bounds = Bounds {
        who: "the command",
        scope: fixing.scope,
        file: &finding.file,
        extents: &extents,
    };
    let standing = Stands { view, sources };
    let after = accept(changes, &bounds, &standing).map_err(|(_, why)| why)?;
    let (edits, bases) = program::edits_of(&after, &standing).map_err(|(_, why)| why)?;
    Ok(Made {
        description: fixing.description.clone(),
        safety: Safety::Safe,
        edits,
        bases,
        moves_only: false,
    })
}

/// Whether the provider of `language` declares `capability`.
fn provides(runtime: &Runtime, language: &str, capability: Capability) -> bool {
    runtime
        .registry()
        .languages()
        .iter()
        .map(|l| l.provider.as_ref())
        .find(|provider| provider.manifest().id == language)
        .is_some_and(|provider| provider.manifest().capabilities.contains(&capability))
}

/// Whether the edits of a fix stay within its scope, else why they do not:
/// the same check as for the files a command or an agent changed.
fn within(
    scope: FixScope,
    finding: &Diagnostic,
    edits: &[TextEdit],
    (view, sources): (&dyn Model, &Sources),
) -> Result<(), String> {
    if scope == FixScope::Files {
        return Ok(());
    }
    let mut changes = Changes::new();
    for file in edits.iter().map(|e| &e.file).collect::<BTreeSet<_>>() {
        let old = sources.text(file)?;
        let of_file: Vec<&TextEdit> = edits.iter().filter(|e| &e.file == file).collect();
        changes.insert(file.clone(), Change::Text(apply(&old, &of_file)?));
    }
    let extents = if scope == FixScope::Symbol {
        vec![choose_extent(
            view,
            finding,
            sources.text(&finding.file)?.as_str(),
        )?]
    } else {
        Vec::new()
    };
    let bounds = Bounds {
        who: "the fix",
        scope,
        file: &finding.file,
        extents: &extents,
    };
    accept(changes, &bounds, &Stands { view, sources })
        .map(|_| ())
        .map_err(|(_, why)| why)
}

/// The text edits of `ops`, and the digest of each file they are in as the fix
/// is made over it.
fn lowered(
    ops: &[EditOp],
    lowerer: &Lowerer,
    sources: &Sources,
) -> Result<(Vec<TextEdit>, BTreeMap<PathBuf, String>), String> {
    let mut edits = Vec::new();
    for op in ops {
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
    if moves_only(ops) {
        for file in edits.iter().map(|e| &e.file).collect::<BTreeSet<_>>() {
            let of_file: Vec<&TextEdit> = edits.iter().filter(|e| &e.file == file).collect();
            keeps_declarations(&sources.text(file)?, &of_file)?;
        }
    }
    let mut bases = BTreeMap::new();
    for edit in &edits {
        if !bases.contains_key(&edit.file) {
            bases.insert(
                edit.file.clone(),
                hash::sha256(sources.text(&edit.file)?.as_str()),
            );
        }
    }
    Ok((edits, bases))
}

/// Whether `ops` only move declarations.
fn moves_only(ops: &[EditOp]) -> bool {
    ops.iter()
        .all(|op| matches!(op, EditOp::Move { .. } | EditOp::Reorder { .. }))
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
