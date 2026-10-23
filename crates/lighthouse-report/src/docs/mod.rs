//! The generated documentation of the decisions: a page per pack, the entry
//! of one decision, and the reference of the fix operations.

mod decision;
mod functions;
mod operations;
mod pack;

pub use decision::{DOCS_DIR, Density, decision_markdown, help_path, pages};
pub use functions::{FunctionDoc, cel_functions_markdown};
pub use operations::{OPERATIONS, Operation, Param, fix_operations_markdown};
