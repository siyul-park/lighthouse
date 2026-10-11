//! The generated documentation of the decisions: a page per pack, the entry
//! of one decision, and the reference of the fix operations.

mod operations;
mod pack;
mod render;

pub use operations::{OPERATIONS, Operation, Param, fix_operations_markdown};
pub use render::{DOCS_DIR, decision_markdown, help_path, pages};
