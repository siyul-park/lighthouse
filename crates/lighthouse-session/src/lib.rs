//! The operations every Lighthouse frontend shares: loading a project, running
//! a check and remembering it, recording judgments, testing and
//! authoring decisions, validating spec documents, and writing
//! the agent skill. The CLI, the MCP server and
//! the agent hooks only parse their own input and print the results of these.

mod authoring;
mod check;
mod decisions;
mod descriptor;
mod findings;
mod generated;
mod git;
mod permit;
mod project;
mod scope;
mod selector;
mod trust;
mod validate;

pub mod baseline;
pub mod environment;
pub mod fix;
pub mod model;
pub mod review;

pub use authoring::{Authored, create_decision, replace_decision};
pub use check::{CheckReport, CheckRequest, Checked, RunStatus, Summary, check};
pub use decisions::{
    DecisionRow, DecisionTest, active_decisions, catalog_index, config_summary, decision_rows,
    decision_text, explain, test_decisions,
};
pub use descriptor::Fixing;
pub use findings::Remembered;
pub use generated::{AGENT_LOOP, SKILL_MARKER, bundled_docs, schema_of, schemas, skill, skill_for};
pub use git::head;
pub use lighthouse_engine::{FailOn, ModelOptions, Timings, cache};
pub use lighthouse_model::{AgentKind, Attribution, Judgment, Severity};
pub use lighthouse_report::docs::DOCS_DIR;
pub use lighthouse_resource::schema_file;
pub use lighthouse_spec::{FILE_NAME, local_dir};
pub use project::{DEFAULT_CONFIG, Session, catalog_at, config_file, project_root};
pub use scope::{changed, since};
pub(crate) use selector::Selector;
pub use trust::{Basis, revoke, trust};
pub use validate::{Problem, Validated, validate_paths};

/// Directories no search for files enters.
pub use lighthouse_engine::SKIPPED_DIRS;

pub use lighthouse_model::Result;

use lighthouse_checks::Composition;

/// The CEL environment of the bundled libraries: built at the composition
/// root every way into the session goes through before a decision is read,
/// and passed on from there.
pub(crate) fn compose() -> Result<Composition> {
    lighthouse_checks::compose().map_err(Into::into)
}
