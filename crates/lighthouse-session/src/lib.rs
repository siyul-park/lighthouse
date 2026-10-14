//! The operations every Lighthouse frontend shares: loading a project, running
//! a check and remembering it, recording judgments, testing and
//! authoring decisions, validating spec documents, and writing
//! the agent skill. The CLI, the MCP server and
//! the agent hooks only parse their own input and print the results of these.

mod authoring;
mod cache;
mod check;
mod decisions;
mod docs;
mod evaluate;
mod findings;
mod fix;
mod git;
mod judged;
mod models;
mod project;
mod reviewing;
mod samples;
mod schema;
mod scope;
mod select;
mod skill;
mod train;
mod trust;
mod validate;

use std::error::Error;

pub use authoring::{Authored, create_decision, replace_decision};
pub use cache::{cache_dir, clean_cache};
pub(crate) use check::attached_models;
pub use check::{CheckRequest, Checked, ModelFlags, RunStatus, Summary, check};
pub use decisions::{
    DecisionRow, DecisionTest, active_decisions, catalog_index, decision_rows, decision_text,
    explain, test_decisions,
};
pub use docs::bundled_docs;
pub use evaluate::eval;
pub use findings::Remembered;
pub use fix::{AppliedRow, DeclinedRow, FixSelection, FixTargets, Fixed, fix};
pub use git::head;
pub use lighthouse_engine::{FailOn, Timings};
pub use lighthouse_ml::{
    evaluate::{EvalReport, EvalRequest, Repo, Side},
    metrics::Rate,
    train::{TrainRequest, Trained},
};
pub use lighthouse_model::{AgentKind, Attribution, Judgment, Severity};
pub use lighthouse_report::docs::DOCS_DIR;
pub use lighthouse_resource::schema_file;
pub use lighthouse_spec::FILE_NAME;
pub use lighthouse_store::{
    FindingRecord, JudgmentEvent, NewJudgment, Standing, StatusFilter, SuppressionEvent,
    evidence_digest,
};
pub use models::{Binding, BoundModel, bound_models, remote_bound};
pub use project::{DEFAULT_CONFIG, Session, catalog_at, config_file, project_root};
pub use reviewing::{
    Recorded, TaskQuery, Tasks, attribute_to, person, record_judgment, review_finding,
    review_history, review_prune, review_tasks,
};
pub use schema::{schema_of, schemas};
pub use scope::{changed, since};
pub(crate) use select::Selector;
pub use select::long_enough;
pub use skill::{SKILL_MARKER, skill, skill_for};
pub use train::train;
pub use trust::{Basis, revoke, trust};
pub use validate::{Problem, Validated, validate_paths};

/// Directories no search for files enters.
pub use lighthouse_engine::SKIPPED_DIRS;

/// Errors cross threads (the MCP server runs operations on a blocking pool).
pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
