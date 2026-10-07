//! The operations every Lighthouse frontend shares: loading a project, running
//! a check and remembering it, recording review verdicts, testing and
//! authoring rules, and writing the agent skill. The CLI, the MCP server and
//! the agent hooks only parse their own input and print the results of these.

mod authoring;
mod check;
mod findings;
mod fix;
mod git;
mod project;
mod reviewing;
mod rules;
mod scope;
mod skill;
mod trust;

use std::error::Error;

pub use authoring::{Authored, create_rule, update_rule};
pub use check::{CheckRequest, Checked, Status, Summary, check};
pub use findings::Remembered;
pub use fix::{AppliedRow, DeclinedRow, FixRequest, Fixed, fix, fix_plan};
pub use git::head;
pub use lighthouse_spec::write_atomic;
pub use project::{DEFAULT_CONFIG, Session, catalog_at, project_root};
pub use reviewing::{Recorded, Reviewer, existing_store, record_verdict};
pub use rules::{RuleRow, RuleTest, explain, rule_rows, test_rules};
pub use scope::{changed, since};
pub use skill::{SKILL_MARKER, skill, skill_for};
pub use trust::{Basis, TRUST_VAR, revoke, trust};

/// Errors cross threads (the MCP server runs operations on a blocking pool).
pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
