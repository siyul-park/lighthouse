//! The operations every Lighthouse frontend shares: loading a project, running
//! a check and remembering it, recording judgments, testing and
//! authoring decisions, validating spec documents, and writing
//! the agent skill. The CLI, the MCP server and
//! the agent hooks only parse their own input and print the results of these.

mod authoring;
mod check;
mod decisions;
mod docs;
mod findings;
mod fix;
mod git;
mod project;
mod reviewing;
mod schema;
mod scope;
mod skill;
mod trust;
mod validate;

use std::error::Error;

pub use authoring::{Authored, create_decision, update_decision};
pub use check::{CheckRequest, Checked, RunStatus, Summary, check};
pub use decisions::{
    DecisionRow, DecisionTest, active_decisions, bundled_decision_rows, catalog_index,
    decision_rows, decision_text, explain, explain_bundled, test_decisions,
};
pub use docs::bundled_docs;
pub use findings::Remembered;
pub use fix::{AppliedRow, DeclinedRow, FixSelection, Fixed, fix, fix_plan};
pub use git::head;
pub use lighthouse_engine::{FailOn, Timings};
pub use lighthouse_model::{AgentKind, Attribution, Judgment, Severity};
pub use lighthouse_resource::schema_file;
pub use lighthouse_spec::{DOCS_DIR, FILE_NAME, write_atomic};
pub use lighthouse_store::{
    FindingRecord, JudgmentEvent, NewJudgment, Standing, StatusFilter, SuppressionEvent,
};
pub use project::{DEFAULT_CONFIG, Session, catalog_at, config_file, project_root};
pub use reviewing::{
    Recorded, Reviewer, TaskQuery, Tasks, record_judgment, review_finding, review_history,
    review_prune, review_tasks,
};
pub use schema::{schema_of, schemas};
pub use scope::{changed, since};
pub use skill::{SKILL_MARKER, skill, skill_for};
pub use trust::{Basis, TRUST_VAR, revoke, trust};
pub use validate::{Problem, Validated, validate_paths};

/// Directories no search for files enters.
pub use lighthouse_engine::SKIPPED_DIRS;

/// Errors cross threads (the MCP server runs operations on a blocking pool).
pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
