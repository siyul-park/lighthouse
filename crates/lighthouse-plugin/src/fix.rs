//! The fixer plugin kind and what it shares with the orchestrator. A rule
//! judges, a fixer proposes, the orchestrator executes: nothing here edits a
//! file.

use lighthouse_model::{
    Diagnostic, Options,
    ucm::{Capability, FixOutcome, Model},
};
use std::{path::Path, sync::Arc};

use serde_json::Value;

use crate::{Error, Queries, Workspace};

/// Static description of a fixer: its identity and what the provider of the
/// file it fixes must offer. A missing capability makes the orchestrator
/// decline before the fixer is asked: a fixer never guesses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixerManifest {
    /// Fully qualified `plugin/name`; the catalog's `fix` of a decision names
    /// the fixer that serves it, and bundled fixers carry their decision's id.
    pub id: String,
    pub requires: Vec<Capability>,
}

/// Everything a fixer may read to answer for one finding. The project is the
/// code model as the graph holds it, and the indexes over it are pulled from
/// the graph: what the fixer reads of them is recorded as what its answer
/// depends on.
pub struct FixRequest<'a> {
    pub finding: &'a Diagnostic,
    /// What the analysis knew about the finding's subject.
    pub facts: &'a Value,
    pub decision: &'a FixDecision,
    /// The options the finding's rule ran with.
    pub options: &'a Options,
    pub project: &'a dyn Model,
    pub queries: &'a dyn Queries,
    pub workspace: &'a Workspace,
    /// The text of `finding.file`.
    pub text: &'a str,
    /// Whether the project is trusted to run commands (`lighthouse trust`, or
    /// `LIGHTHOUSE_TRUST=1`); the repository cannot grant it itself.
    pub trusted: bool,
    /// What the fixer built for the file of the finding with
    /// [`Fixer::values`], once for all its findings; `Null` when it builds none.
    pub values: &'a Arc<Value>,
}

/// Everything a fixer may read to build what its fixes of one file share,
/// read as a [`FixRequest`] is.
pub struct ValuesRequest<'a> {
    pub file: &'a Path,
    /// The options the rule ran with on the file.
    pub options: &'a Options,
    pub project: &'a dyn Model,
    pub queries: &'a dyn Queries,
    pub workspace: &'a Workspace,
    /// The text of `file`.
    pub text: &'a str,
}

/// The decision behind a finding, as far as a fixer may use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixDecision {
    pub id: String,
    pub requirement: String,
    pub context: String,
}

/// Proposes the edits that make a finding go away, as [`FixOutcome`]. `fix`
/// must be deterministic for equal inputs and must not touch the file system
/// outside a scratch copy.
pub trait Fixer: Send + Sync {
    fn manifest(&self) -> &FixerManifest;
    fn fix(&self, request: &FixRequest) -> Result<FixOutcome, Error>;

    /// What the fixes of one file share, which depends on the file and not on
    /// the finding: built once and handed to each as [`FixRequest::values`].
    /// `Null` when the fixer shares nothing.
    fn values(&self, _request: &ValuesRequest) -> Result<Value, Error> {
        Ok(Value::Null)
    }
}
