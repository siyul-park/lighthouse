//! Orchestration of one run: index the project, analyze, check, suppress what
//! its annotations say, and apply fixes in rounds. It never touches the store
//! or a frontend; the composition root hands it a registry and a catalog.

mod annotations;
mod documents;
mod engine;
mod error;
mod facts;
mod fix;
mod generated;
mod graph;
mod tester;
mod timings;
mod tools;

pub mod cache;

pub use engine::{
    Availability, CommandModels, Conflicts, EXIT_INCOMPLETE, Engine, FailOn, ModelOptions, Needs,
    Orders, Outcome, Overlays, Run, active,
};
pub use error::Error;
pub use fix::{
    AppliedFix, BatchTrace, DeclinedFix, EditUsage, FileChange, FixBinding, FixPlan, FixReport,
    FixRun, FixedBy, MAX_ROUNDS, ModelCounts, PreviewedFix, ToolCounts, brief, unified_diff,
};
pub use tester::{
    Alphabet, Arrangement, Basis, Bounded, Cell, Contradiction, Decl, Edge, ExampleRun, Flagged,
    Forced, Incomplete, Kind, Language, MAX_BOUND, Oracle, Plant, Planted, Relation, RuleTester,
    Scratch, Searched, Shape, Single, Spent, Tested, Unit, Verdict, Visibility, Witness, enumerate,
    local_decisions,
};
pub use timings::{Detect, FixTimings, Timings};
