//! The operations every Lighthouse frontend shares: loading a project, running
//! a check and remembering it, recording judgments, testing and
//! authoring decisions, validating spec documents, and writing
//! the agent skill. The CLI, the MCP server and
//! the agent hooks only parse their own input and print the results of these.

mod assemble;
mod authoring;
mod cache;
mod check;
mod dataset;
mod decisions;
mod docs;
pub mod evaluate;
mod findings;
mod fix;
mod git;
pub mod labels;
pub mod metrics;
mod models;
mod project;
mod reviewing;
mod samples;
mod schema;
mod scope;
mod select;
mod skill;
pub mod train;
mod trust;
mod validate;

use std::error::Error;

pub use authoring::{Authored, create_decision, replace_decision};
pub use cache::{LIMIT_VAR, cache_dir, clean_cache};
pub use check::{CheckRequest, Checked, ModelFlags, RunStatus, Summary, attached_models, check};
pub use decisions::{
    DecisionRow, DecisionTest, active_decisions, bundled_decision_rows, catalog_index,
    decision_rows, decision_text, explain, explain_bundled, test_decisions,
};
pub use docs::bundled_docs;
pub use findings::Remembered;
pub use fix::{AppliedRow, DeclinedRow, FixSelection, FixTargets, Fixed, fix, fix_plan};
pub use git::head;
pub use lighthouse_engine::{FailOn, Timings};
pub use lighthouse_model::{AgentKind, Attribution, Judgment, Severity};
pub use lighthouse_resource::schema_file;
pub use lighthouse_spec::{DOCS_DIR, FILE_NAME};
pub use lighthouse_store::{
    FindingRecord, JudgmentEvent, NewJudgment, Standing, StatusFilter, SuppressionEvent,
};
pub use models::{Binding, BoundModel, bound_models, remote_bound};
pub use project::{DEFAULT_CONFIG, Session, catalog_at, config_file, project_root};
pub use reviewing::{
    Recorded, TaskQuery, Tasks, attribute_to, person, record_judgment, review_finding,
    review_history, review_prune, review_tasks,
};
pub use schema::{schema_of, schemas};
pub use scope::{changed, since};
pub use select::{Selector, long_enough};
pub use skill::{SKILL_MARKER, skill, skill_for};
pub use trust::{Basis, TRUST_VAR, revoke, trust};
pub use validate::{Problem, Validated, validate_paths};

/// Directories no search for files enters.
pub use lighthouse_engine::SKIPPED_DIRS;

/// Errors cross threads (the MCP server runs operations on a blocking pool).
pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
