//! Orchestration of one run: index the project, analyze, check, suppress what
//! its annotations say, and apply fixes in rounds. It never touches the store
//! or a frontend; the composition root hands it a registry and a catalog.

mod annotations;
mod documents;
mod engine;
mod error;
mod facts;
mod fix;
mod tester;
mod timings;

pub mod cache;

pub use documents::SKIPPED_DIRS;
pub use engine::{
    Availability, CommandModels, EXIT_INCOMPLETE, Engine, FailOn, ModelOptions, Needs, Overlays,
    Run, active,
};
pub use error::Error;
pub use fix::{
    AppliedFix, BatchTrace, DeclinedFix, EditUsage, FileChange, FixBinding, FixPlan, FixReport,
    FixRun, FixedBy, MAX_ROUNDS, ModelCounts, PreviewedFix, ToolCounts, brief, unified_diff,
};
pub use tester::{ExampleRun, RuleTester};
pub use timings::Timings;
