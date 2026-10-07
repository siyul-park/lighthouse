//! The fixer plugin kind and what it shares with the orchestrator. A rule
//! judges, a fixer proposes, the orchestrator executes: nothing here edits a
//! file.

use lighthouse_model::{Capability, Diagnostic, FixOutcome, Options, Project, Symbol};
use serde_json::Value;

use crate::{Error, Workspace};

/// Static description of a fixer: its identity and what the provider of the
/// file it fixes must offer. A missing capability makes the orchestrator
/// decline before the fixer is asked: a fixer never guesses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixerManifest {
    /// Fully qualified `plugin/name`; the catalog's `fix` of a pattern names
    /// the fixer that serves it, and bundled fixers carry their pattern's id.
    pub id: String,
    pub requires: Vec<Capability>,
}

/// The pattern behind a finding, as far as a fixer may use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixPattern {
    pub id: String,
    pub requirement: String,
    pub intent: String,
}

/// Everything a fixer may read to answer for one finding. The project is the
/// analysis the finding came from, read-only.
pub struct FixRequest<'a> {
    pub finding: &'a Diagnostic,
    /// What the analysis knew about the finding's subject.
    pub facts: &'a Value,
    pub pattern: &'a FixPattern,
    /// The options the finding's rule ran with.
    pub options: &'a Options,
    pub project: &'a Project,
    pub ws: &'a Workspace,
    /// The text of `finding.file`.
    pub text: &'a str,
    /// The registered order keys of the run's plugins.
    pub keys: &'a dyn OrderKeys,
    /// Whether the project is trusted to run commands (`lighthouse trust`, or
    /// `LIGHTHOUSE_TRUST=1`); the repository cannot grant it itself.
    pub trusted: bool,
}

/// Proposes the edits that make a finding go away, as [`FixOutcome`]. `fix`
/// must be deterministic for equal inputs and must not touch the file system
/// outside a scratch copy.
pub trait Fixer: Send + Sync {
    fn manifest(&self) -> &FixerManifest;
    fn fix(&self, request: &FixRequest) -> Result<FixOutcome, Error>;
}

/// What an order key reads when it ranks a declaration.
pub struct KeyCtx<'a> {
    pub project: &'a Project,
    /// Language of the file being reordered.
    pub language: &'a str,
    /// The rule whose finding is being fixed, and the options it ran with.
    pub rule: &'a str,
    pub options: &'a Options,
}

/// A way to order declarations, for the `reorder` fix operation. `rank` is
/// `None` for a declaration the key does not order: it stays where it is.
pub trait OrderKey: Send + Sync {
    fn manifest(&self) -> &OrderKeyManifest;
    fn rank(&self, ctx: &KeyCtx, symbol: &Symbol) -> Result<Option<u64>, Error>;
}

/// What an order key declares about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderKeyManifest {
    /// Fully qualified `plugin/name`.
    pub id: String,
    /// What the key orders by, for the generated docs.
    pub description: String,
}

/// The order keys a run can use, by id.
pub trait OrderKeys: Send + Sync {
    fn key(&self, id: &str) -> Option<&dyn OrderKey>;
}
