//! The operations every Lighthouse frontend shares: loading a project, running
//! a check and remembering it, recording review verdicts, testing and
//! authoring decisions, migrating and validating spec documents, and writing
//! the agent skill. The CLI, the MCP server and
//! the agent hooks only parse their own input and print the results of these.

mod authoring;
mod check;
mod decisions;
mod findings;
mod fix;
mod git;
pub mod migrate;
mod project;
mod reviewing;
mod schema;
mod scope;
mod skill;
mod trust;
mod validate;

use std::error::Error;

pub use authoring::{Authored, create_decision, update_decision};
pub use check::{CheckRequest, Checked, Status, Summary, check};
pub use decisions::{DecisionRow, DecisionTest, decision_rows, explain, test_decisions};
pub use findings::Remembered;
pub use fix::{AppliedRow, DeclinedRow, FixRequest, Fixed, fix, fix_plan};
pub use git::head;
pub use lighthouse_spec::write_atomic;
pub use migrate::{Migrated, migrate_paths};
pub use project::{DEFAULT_CONFIG, Session, catalog_at, project_root};
pub use reviewing::{Recorded, Reviewer, existing_store, record_verdict};
pub use schema::{schema_of, schemas};
pub use scope::{changed, since};
pub use skill::{SKILL_MARKER, skill, skill_for};
pub use trust::{Basis, TRUST_VAR, revoke, trust};
pub use validate::{Problem, Validated, validate_paths};

/// Directories no search for files enters.
pub const SKIPPED_DIRS: [&str; 4] = [".git", "target", "node_modules", "testdata"];

/// Errors cross threads (the MCP server runs operations on a blocking pool).
pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
